//! Public, unauthenticated endpoints: server info, manifests, chunks, torrents
//! and the web-seed file layout.

use crate::{
    db,
    error::{ApiError, ApiResult},
    games,
    metrics::Metrics,
    state::St,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use gamedist_core::{
    api::{CatalogPage, ReleaseList},
    manifest::{GameManifest, ReleaseManifest, ServerInfo},
    ChunkId, PROTOCOL,
};
use serde::Deserialize;
use std::sync::Arc;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub async fn server_info(State(st): State<St>) -> ApiResult<Json<ServerInfo>> {
    Ok(Json(ServerInfo {
        protocol: PROTOCOL.to_string(),
        name: st.cfg.name.clone(),
        api: format!("{}/api/v0", st.cfg.public_url),
        chunks: st.cfg.chunks_base(),
        tracker: st.cfg.tracker_url(),
        catalog: st
            .cfg
            .catalog
            .then(|| format!("{}/api/v0/games", st.cfg.public_url)),
        registration: db::registration_mode(&st.db, st.cfg.registration).await?,
    }))
}

#[derive(Debug, Deserialize)]
pub struct CatalogQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    page: Option<u32>,
}

pub async fn catalog(
    State(st): State<St>,
    Query(query): Query<CatalogQuery>,
) -> ApiResult<Json<CatalogPage>> {
    if !st.cfg.catalog {
        return Err(ApiError::not_found("this server does not expose a catalog"));
    }
    let page = query.page.unwrap_or(1).max(1);
    let (rows, total) = games::catalog(&st, &query.q, page).await?;
    let mut list = Vec::with_capacity(rows.len());
    for row in &rows {
        list.push(games::summary_for(&st, row).await?);
    }
    Ok(Json(CatalogPage {
        games: list,
        page,
        pages: (total.div_ceil(games::PAGE_SIZE as u64) as u32).max(1),
        total,
    }))
}

/// `GET /api/v0/export.jsonl` — every public game manifest, one per line.
pub async fn export(State(st): State<St>) -> ApiResult<Response> {
    if !st.cfg.catalog {
        return Err(ApiError::not_found("this server does not expose a catalog"));
    }
    let rows = sqlx::query("SELECT * FROM games WHERE taken_down = 0 ORDER BY id")
        .fetch_all(&st.db)
        .await?;
    let mut out = String::new();
    for row in &rows {
        let manifest = games::manifest_for(&st, &db::GameRow::from_row(row)).await?;
        out.push_str(
            &serde_json::to_string(&manifest).map_err(|e| ApiError::internal("export", e))?,
        );
        out.push('\n');
    }
    Ok(([(header::CONTENT_TYPE, "application/jsonl")], out).into_response())
}

pub async fn game_manifest(
    State(st): State<St>,
    Path(slug): Path<String>,
) -> ApiResult<Json<GameManifest>> {
    let game = games::public_game(&st, &slug).await?;
    Ok(Json(games::manifest_for(&st, &game).await?))
}

pub async fn release_list(
    State(st): State<St>,
    Path(slug): Path<String>,
) -> ApiResult<Json<ReleaseList>> {
    let game = games::public_game(&st, &slug).await?;
    Ok(Json(games::release_list(&st, &game).await?))
}

fn manifest_response(text: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            // Immutable in principle, but a release can be unpublished.
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        text,
    )
        .into_response()
}

/// `GET /api/v0/games/{slug}/releases/{version}`
pub async fn release_api(
    State(st): State<St>,
    Path((slug, version)): Path<(String, String)>,
) -> ApiResult<Response> {
    let game = games::public_game(&st, &slug).await?;
    Ok(manifest_response(
        games::release_text(&st, &game, &version).await?,
    ))
}

/// `GET /games/{slug}/releases/{file}` where `file` is `<version>.json` or
/// `<version>-<platform>.torrent`.
pub async fn release_file(
    State(st): State<St>,
    Path((slug, file)): Path<(String, String)>,
) -> ApiResult<Response> {
    let game = games::public_game(&st, &slug).await?;
    if let Some(version) = file.strip_suffix(".json") {
        return Ok(manifest_response(
            games::release_text(&st, &game, version).await?,
        ));
    }
    let Some(stem) = file.strip_suffix(".torrent") else {
        return Err(ApiError::not_found(file));
    };
    // Versions and platforms both contain dashes, so match against what exists.
    let rows: Vec<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT r.version, t.platform, t.data FROM torrents t
         JOIN releases r ON r.id = t.release_id WHERE r.game_id = ?",
    )
    .bind(game.id)
    .fetch_all(&st.db)
    .await?;
    let data = rows
        .into_iter()
        .find(|(version, platform, _)| stem == format!("{version}-{platform}"))
        .map(|(_, _, data)| data)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "{file} (torrents are generated shortly after a release is published)"
            ))
        })?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-bittorrent".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{slug}-{file}\""),
            ),
        ],
        data,
    )
        .into_response())
}

pub async fn chunk(
    State(st): State<St>,
    method: Method,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let id: ChunkId = id.parse()?;
    let size = st
        .chunks
        .size(&id)
        .ok_or_else(|| ApiError::not_found(format!("chunk {id}")))?;
    let headers = [
        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        (header::CACHE_CONTROL, IMMUTABLE.to_string()),
        (header::CONTENT_LENGTH, size.to_string()),
        (header::ETAG, format!("\"{id}\"")),
    ];
    if method == Method::HEAD {
        return Ok((headers, Body::empty()).into_response());
    }
    let store = st.chunks.clone();
    let data = tokio::task::spawn_blocking(move || store.get_unverified(&id))
        .await
        .map_err(|e| ApiError::internal("chunk read task", e))??;
    Metrics::add(&st.metrics.chunks_served, 1);
    Metrics::add(&st.metrics.chunk_bytes_served, data.len() as u64);
    Ok((headers, data).into_response())
}

async fn load_manifest(st: &St, slug: &str, version: &str) -> ApiResult<Arc<ReleaseManifest>> {
    let game = games::public_game(st, slug).await?;
    if let Some(cached) = st.cached_manifest(slug, version) {
        return Ok(cached);
    }
    let text = games::release_text(st, &game, version).await?;
    let manifest: ReleaseManifest =
        serde_json::from_str(&text).map_err(|e| ApiError::internal("stored manifest", e))?;
    let manifest = Arc::new(manifest);
    st.cache_manifest(slug, version, manifest.clone());
    Ok(manifest)
}

/// Parse a single-range `Range` header. `Ok(None)` means "serve everything".
fn parse_range(headers: &HeaderMap, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(value) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let Some(spec) = value.strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None); // multi-range: answer with the whole file
    }
    let (start, end) = spec.split_once('-').ok_or(())?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
            (size.saturating_sub(n), size.saturating_sub(1))
        }
        (start, "") => (start.parse().map_err(|_| ())?, size.saturating_sub(1)),
        (start, end) => (
            start.parse().map_err(|_| ())?,
            end.parse::<u64>()
                .map_err(|_| ())?
                .min(size.saturating_sub(1)),
        ),
    };
    if size == 0 || start > end || start >= size {
        return Err(());
    }
    Ok(Some((start, end)))
}

/// `GET /games/{slug}/releases/{version}/{platform}/files/{*path}` — the
/// assembled file, streamed chunk by chunk, with `Range` support.
pub async fn web_seed_file(
    State(st): State<St>,
    method: Method,
    Path((slug, version, platform, path)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let manifest = load_manifest(&st, &slug, &version).await?;
    let file = manifest
        .platforms
        .get(&platform)
        .and_then(|p| p.file(&path))
        .ok_or_else(|| ApiError::not_found(format!("{platform}/{path}")))?
        .clone();

    let range = match parse_range(&headers, file.size) {
        Ok(range) => range,
        Err(()) => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{}", file.size))],
            )
                .into_response())
        }
    };
    let (start, end, status) = match range {
        Some((start, end)) => (start, end, StatusCode::PARTIAL_CONTENT),
        None => (0, file.size.saturating_sub(1), StatusCode::OK),
    };
    let length = if file.size == 0 { 0 } else { end - start + 1 };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length)
        .header(header::CACHE_CONTROL, "public, max-age=86400");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{}", file.size),
        );
    }
    if method == Method::HEAD || length == 0 {
        return builder
            .body(Body::empty())
            .map_err(|e| ApiError::internal("response", e));
    }

    // Work out which slice of which chunk is needed.
    let mut parts = Vec::new();
    let mut offset = 0u64;
    for chunk in &file.chunks {
        let chunk_end = offset + chunk.size; // exclusive
        if chunk_end > start && offset <= end {
            let from = start.saturating_sub(offset) as usize;
            let to = (end + 1).min(chunk_end) - offset;
            parts.push((chunk.id, from, to as usize));
        }
        offset = chunk_end;
        if offset > end {
            break;
        }
    }
    let state = st.clone();
    let stream = futures_util::stream::iter(parts).then(move |(id, from, to)| {
        let state = state.clone();
        async move {
            let store = state.chunks.clone();
            let data = tokio::task::spawn_blocking(move || store.get_unverified(&id))
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            if to > data.len() {
                return Err(std::io::Error::other(
                    "stored chunk is shorter than declared",
                ));
            }
            Metrics::add(&state.metrics.file_bytes_served, (to - from) as u64);
            Ok::<_, std::io::Error>(bytes::Bytes::from(data).slice(from..to))
        }
    });
    builder
        .body(Body::from_stream(stream))
        .map_err(|e| ApiError::internal("response", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(value: &str, size: u64) -> Result<Option<(u64, u64)>, ()> {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, value.parse().unwrap());
        parse_range(&headers, size)
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range(&HeaderMap::new(), 100), Ok(None));
        assert_eq!(range("bytes=0-9", 100), Ok(Some((0, 9))));
        assert_eq!(range("bytes=90-", 100), Ok(Some((90, 99))));
        assert_eq!(range("bytes=-10", 100), Ok(Some((90, 99))));
        assert_eq!(range("bytes=-500", 100), Ok(Some((0, 99))));
        assert_eq!(range("bytes=50-5000", 100), Ok(Some((50, 99))));
        assert_eq!(range("bytes=0-1,5-6", 100), Ok(None));
        assert_eq!(range("items=0-1", 100), Ok(None));
        assert_eq!(range("bytes=100-", 100), Err(()));
        assert_eq!(range("bytes=9-3", 100), Err(()));
        assert_eq!(range("bytes=abc", 100), Err(()));
        assert_eq!(range("bytes=0-", 0), Err(()));
    }
}
