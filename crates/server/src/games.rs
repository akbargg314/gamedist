//! Game and release operations shared by the REST API and the web portal.

use crate::{
    auth::User,
    db::{self, GameRow},
    error::{ApiError, ApiResult},
    metrics::Metrics,
    state::{AppState, St},
    torrents,
};
use gamedist_core::{
    api::{GameSummary, ReleaseList, ReleaseSummary, TorrentRef},
    game_id, ids,
    manifest::{Assets, GameManifest, KeyRotation, SignedRelease},
    ChunkId, PublicKey, PROTOCOL,
};
use sqlx::Row;
use std::collections::{BTreeMap, HashMap};

/// Fetch a game for public consumption: 404 if unknown, 451 if taken down.
pub async fn public_game(st: &AppState, slug: &str) -> ApiResult<GameRow> {
    let game = db::game_by_slug(&st.db, slug)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("game {slug}")))?;
    if game.taken_down {
        return Err(ApiError::taken_down(&game.takedown_reason));
    }
    Ok(game)
}

/// Fetch a game the user may modify (owner or admin).
pub async fn owned_game(st: &AppState, user: &User, slug: &str) -> ApiResult<GameRow> {
    let game = db::game_by_slug(&st.db, slug)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("game {slug}")))?;
    if game.owner_id != user.id && !user.is_admin {
        return Err(ApiError::forbidden("you do not own this game"));
    }
    if game.taken_down && !user.is_admin {
        return Err(ApiError::taken_down(&game.takedown_reason));
    }
    Ok(game)
}

fn parse_pubkey(game: &GameRow) -> ApiResult<PublicKey> {
    game.developer_pubkey
        .parse()
        .map_err(|e| ApiError::internal("stored pubkey", e))
}

pub async fn manifest_for(st: &AppState, game: &GameRow) -> ApiResult<GameManifest> {
    Ok(GameManifest {
        protocol: PROTOCOL.to_string(),
        game_id: game.game_id.clone(),
        slug: game.slug.clone(),
        developer_pubkey: parse_pubkey(game)?,
        title: game.title.clone(),
        summary: game.summary.clone(),
        description_md: game.description_md.clone(),
        assets: serde_json::from_str(&game.assets_json).unwrap_or_default(),
        channels: db::channels(&st.db, game.id).await?,
        releases: format!("{}/games/{}/releases/", st.cfg.public_url, game.slug),
        rotations: serde_json::from_str(&game.rotations_json).unwrap_or_default(),
    })
}

pub async fn summary_for(st: &AppState, game: &GameRow) -> ApiResult<GameSummary> {
    let assets: Assets = serde_json::from_str(&game.assets_json).unwrap_or_default();
    Ok(GameSummary {
        game_id: game.game_id.clone(),
        slug: game.slug.clone(),
        title: game.title.clone(),
        summary: game.summary.clone(),
        developer_pubkey: parse_pubkey(game)?,
        url: st.game_url(&game.slug),
        icon: assets.icon,
        banner: assets.banner,
        channels: db::channels(&st.db, game.id).await?,
    })
}

pub const PAGE_SIZE: u32 = 24;

/// One page of the public catalog. Returns `(games, total)`.
pub async fn catalog(st: &AppState, query: &str, page: u32) -> ApiResult<(Vec<GameRow>, u64)> {
    let like = format!(
        "%{}%",
        query
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let filter = "taken_down = 0 AND (title LIKE ?1 ESCAPE '\\' OR slug LIKE ?1 ESCAPE '\\' OR summary LIKE ?1 ESCAPE '\\')";
    let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM games WHERE {filter}"))
        .bind(&like)
        .fetch_one(&st.db)
        .await?;
    let rows = sqlx::query(&format!(
        "SELECT * FROM games WHERE {filter} ORDER BY updated_at DESC, id DESC LIMIT ?2 OFFSET ?3"
    ))
    .bind(&like)
    .bind(PAGE_SIZE)
    .bind(page.saturating_sub(1).saturating_mul(PAGE_SIZE))
    .fetch_all(&st.db)
    .await?;
    Ok((rows.iter().map(GameRow::from_row).collect(), total as u64))
}

pub fn validate_text(what: &str, value: &str, max: usize, required: bool) -> ApiResult<()> {
    if required && value.trim().is_empty() {
        return Err(ApiError::bad_request(format!("{what} must not be empty")));
    }
    if value.chars().count() > max {
        return Err(ApiError::bad_request(format!(
            "{what} is longer than {max} characters"
        )));
    }
    Ok(())
}

pub async fn register_key(
    st: &AppState,
    user: &User,
    key: PublicKey,
    label: &str,
) -> ApiResult<bool> {
    validate_text("label", label, 100, false)?;
    let result = sqlx::query(
        "INSERT INTO dev_keys (user_id, pubkey, label, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(user_id, pubkey) DO NOTHING",
    )
    .bind(user.id)
    .bind(key.to_string())
    .bind(label)
    .bind(db::now())
    .execute(&st.db)
    .await?;
    let created = result.rows_affected() > 0;
    if created {
        db::audit(&st.db, user.actor(), "key.add", &key.to_string(), label).await;
    }
    Ok(created)
}

async fn user_has_key(st: &AppState, user: &User, key: &PublicKey) -> ApiResult<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM dev_keys WHERE user_id = ? AND pubkey = ?")
            .bind(user.id)
            .bind(key.to_string())
            .fetch_optional(&st.db)
            .await?;
    Ok(found.is_some())
}

pub async fn create_game(
    st: &AppState,
    user: &User,
    slug: &str,
    key: PublicKey,
    title: &str,
    summary: &str,
    description_md: &str,
) -> ApiResult<GameRow> {
    ids::validate_slug(slug)?;
    validate_text("title", title, 200, true)?;
    validate_text("summary", summary, 300, false)?;
    validate_text("description", description_md, 100_000, false)?;
    if !user_has_key(st, user, &key).await? {
        return Err(ApiError::forbidden(
            "that developer key is not registered to your account; POST it to /dev/keys first",
        ));
    }
    let now = db::now();
    let result = sqlx::query(
        "INSERT INTO games (slug, game_id, owner_id, developer_pubkey, title, summary, description_md, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(slug)
    .bind(game_id(&key, slug))
    .bind(user.id)
    .bind(key.to_string())
    .bind(title.trim())
    .bind(summary.trim())
    .bind(description_md)
    .bind(now)
    .bind(now)
    .execute(&st.db)
    .await;
    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::conflict(format!(
                "the slug {slug:?} is already taken on this server"
            )))
        }
        Err(e) => return Err(e.into()),
    }
    db::audit(&st.db, user.actor(), "game.create", slug, "").await;
    db::game_by_slug(&st.db, slug)
        .await?
        .ok_or_else(|| ApiError::internal("create game", "row vanished"))
}

pub async fn update_game(
    st: &AppState,
    user: &User,
    slug: &str,
    title: Option<&str>,
    summary: Option<&str>,
    description_md: Option<&str>,
    assets: Option<&Assets>,
) -> ApiResult<GameRow> {
    let game = owned_game(st, user, slug).await?;
    let mut tx = st.db.begin().await?;
    if let Some(title) = title {
        validate_text("title", title, 200, true)?;
        sqlx::query("UPDATE games SET title = ? WHERE id = ?")
            .bind(title.trim())
            .bind(game.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(summary) = summary {
        validate_text("summary", summary, 300, false)?;
        sqlx::query("UPDATE games SET summary = ? WHERE id = ?")
            .bind(summary.trim())
            .bind(game.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(description) = description_md {
        validate_text("description", description, 100_000, false)?;
        sqlx::query("UPDATE games SET description_md = ? WHERE id = ?")
            .bind(description)
            .bind(game.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(assets) = assets {
        if assets.screenshots.len() > 32 {
            return Err(ApiError::bad_request("at most 32 screenshots"));
        }
        for id in assets.chunk_ids() {
            let found: Option<i64> = sqlx::query_scalar("SELECT 1 FROM chunks WHERE id = ?")
                .bind(id.to_hex())
                .fetch_optional(&mut *tx)
                .await?;
            if found.is_none() {
                return Err(ApiError::bad_request(format!(
                    "asset chunk {id} has not been uploaded"
                )));
            }
        }
        sqlx::query("DELETE FROM game_assets WHERE game_id = ?")
            .bind(game.id)
            .execute(&mut *tx)
            .await?;
        for id in assets.chunk_ids() {
            sqlx::query("INSERT OR IGNORE INTO game_assets (game_id, chunk_id) VALUES (?, ?)")
                .bind(game.id)
                .bind(id.to_hex())
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE games SET assets_json = ? WHERE id = ?")
            .bind(serde_json::to_string(assets).map_err(|e| ApiError::internal("assets", e))?)
            .bind(game.id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE games SET updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(game.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    db::audit(&st.db, user.actor(), "game.update", slug, "").await;
    db::game_by_slug(&st.db, slug)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("game {slug}")))
}

/// Verify and store a signed release. Returns `true` if newly created.
pub async fn publish_release(st: &St, user: &User, slug: &str, text: &str) -> ApiResult<bool> {
    let game = owned_game(st, user, slug).await?;
    let release = SignedRelease::parse(text)?;
    let manifest = release.manifest();
    if manifest.slug != game.slug {
        return Err(ApiError::bad_request(format!(
            "manifest is for {:?}, not {:?}",
            manifest.slug, game.slug
        )));
    }
    if manifest.developer_pubkey.to_string() != game.developer_pubkey {
        return Err(ApiError::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "Wrong developer key",
            "the release is not signed by this game's developer key",
        ));
    }

    let existing: Option<String> =
        sqlx::query_scalar("SELECT manifest FROM releases WHERE game_id = ? AND version = ?")
            .bind(game.id)
            .bind(&manifest.version)
            .fetch_optional(&st.db)
            .await?;
    if let Some(existing) = existing {
        return if existing == text {
            Ok(false)
        } else {
            Err(ApiError::conflict(format!(
                "version {} is already published and releases are immutable; bump the version",
                manifest.version
            )))
        };
    }

    // Every referenced chunk must be here, with the declared size.
    let chunks = manifest.unique_chunks();
    let mut known: HashMap<String, i64> = HashMap::with_capacity(chunks.len());
    for batch in chunks.chunks(500) {
        let placeholders = vec!["?"; batch.len()].join(",");
        let sql = format!("SELECT id, size FROM chunks WHERE id IN ({placeholders})");
        let mut query = sqlx::query(&sql);
        for chunk in batch {
            query = query.bind(chunk.id.to_hex());
        }
        for row in query.fetch_all(&st.db).await? {
            known.insert(row.get("id"), row.get("size"));
        }
    }
    let mut missing = Vec::new();
    for chunk in &chunks {
        match known.get(&chunk.id.to_hex()) {
            Some(size) if *size as u64 == chunk.size => {}
            Some(size) => {
                return Err(ApiError::bad_request(format!(
                    "chunk {} is {size} bytes on the server but the manifest says {}",
                    chunk.id, chunk.size
                )))
            }
            None => missing.push(chunk.id),
        }
    }
    if !missing.is_empty() {
        return Err(ApiError::bad_request(format!(
            "{} referenced chunk(s) have not been uploaded (first: {})",
            missing.len(),
            missing[0]
        )));
    }

    let platforms: Vec<&str> = manifest.platforms.keys().map(String::as_str).collect();
    let total: u64 = manifest.platforms.values().map(|p| p.total_size).sum();
    let mut tx = st.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO releases (game_id, version, manifest, created, platforms, total_size, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(game.id)
    .bind(&manifest.version)
    .bind(text)
    .bind(&manifest.created)
    .bind(platforms.join(","))
    .bind(total as i64)
    .bind(db::now())
    .execute(&mut *tx)
    .await;
    let release_id = match inserted {
        Ok(done) => done.last_insert_rowid(),
        // Lost a race with an identical concurrent publish.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::conflict("this version was just published"))
        }
        Err(e) => return Err(e.into()),
    };
    for chunk in &chunks {
        sqlx::query("INSERT INTO release_chunks (release_id, chunk_id) VALUES (?, ?)")
            .bind(release_id)
            .bind(chunk.id.to_hex())
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE games SET updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(game.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Metrics::add(&st.metrics.releases_published, 1);
    db::audit(
        &st.db,
        user.actor(),
        "release.publish",
        &format!("{slug}@{}", manifest.version),
        &platforms.join(","),
    )
    .await;

    // Torrent generation hashes the whole release; do it in the background.
    let state = st.clone();
    let manifest = manifest.clone();
    tokio::spawn(async move {
        if let Err(e) = torrents::generate(&state, release_id, &manifest).await {
            tracing::error!(
                "generating torrents for {}@{}: {}",
                manifest.slug,
                manifest.version,
                e.detail
            );
        }
    });
    Ok(true)
}

pub async fn set_channel(
    st: &AppState,
    user: &User,
    slug: &str,
    channel: &str,
    version: &str,
) -> ApiResult<()> {
    let game = owned_game(st, user, slug).await?;
    ids::validate_channel(channel)?;
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM releases WHERE game_id = ? AND version = ?")
            .bind(game.id)
            .bind(version)
            .fetch_optional(&st.db)
            .await?;
    if exists.is_none() {
        return Err(ApiError::not_found(format!("release {slug} {version}")));
    }
    sqlx::query(
        "INSERT INTO channels (game_id, name, version) VALUES (?, ?, ?)
         ON CONFLICT(game_id, name) DO UPDATE SET version = excluded.version",
    )
    .bind(game.id)
    .bind(channel)
    .bind(version)
    .execute(&st.db)
    .await?;
    sqlx::query("UPDATE games SET updated_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(game.id)
        .execute(&st.db)
        .await?;
    db::audit(
        &st.db,
        user.actor(),
        "channel.set",
        slug,
        &format!("{channel} -> {version}"),
    )
    .await;
    Ok(())
}

pub async fn delete_release(
    st: &AppState,
    user: &User,
    slug: &str,
    version: &str,
) -> ApiResult<()> {
    let game = owned_game(st, user, slug).await?;
    let mut tx = st.db.begin().await?;
    let deleted = sqlx::query("DELETE FROM releases WHERE game_id = ? AND version = ?")
        .bind(game.id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found(format!("release {slug} {version}")));
    }
    sqlx::query("DELETE FROM channels WHERE game_id = ? AND version = ?")
        .bind(game.id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    st.evict_manifests(slug);
    db::audit(
        &st.db,
        user.actor(),
        "release.unpublish",
        &format!("{slug}@{version}"),
        "",
    )
    .await;
    Ok(())
}

/// Accept a key-rotation statement: the game moves to the new key.
pub async fn rotate_key(
    st: &AppState,
    user: &User,
    slug: &str,
    statement: &serde_json::Value,
) -> ApiResult<()> {
    let game = owned_game(st, user, slug).await?;
    let rotation = KeyRotation::verify(statement)?;
    if rotation.slug != game.slug {
        return Err(ApiError::bad_request("statement is for a different game"));
    }
    if rotation.old_key.to_string() != game.developer_pubkey {
        return Err(ApiError::bad_request(
            "statement's old_key is not this game's current developer key",
        ));
    }
    if !user_has_key(st, user, &rotation.new_key).await? {
        return Err(ApiError::forbidden(
            "the new key is not registered to your account; POST it to /dev/keys first",
        ));
    }
    let mut rotations: Vec<serde_json::Value> =
        serde_json::from_str(&game.rotations_json).unwrap_or_default();
    rotations.push(statement.clone());
    sqlx::query(
        "UPDATE games SET developer_pubkey = ?, game_id = ?, rotations_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(rotation.new_key.to_string())
    .bind(game_id(&rotation.new_key, slug))
    .bind(serde_json::to_string(&rotations).map_err(|e| ApiError::internal("rotations", e))?)
    .bind(db::now())
    .bind(game.id)
    .execute(&st.db)
    .await?;
    // Channels point at releases signed by the old key; they must be
    // re-published under the new key.
    sqlx::query("DELETE FROM channels WHERE game_id = ?")
        .bind(game.id)
        .execute(&st.db)
        .await?;
    db::audit(
        &st.db,
        user.actor(),
        "key.rotate",
        slug,
        &format!("{} -> {}", rotation.old_key, rotation.new_key),
    )
    .await;
    Ok(())
}

pub async fn release_list(st: &AppState, game: &GameRow) -> ApiResult<ReleaseList> {
    let rows = sqlx::query(
        "SELECT id, version, created, platforms FROM releases WHERE game_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(game.id)
    .fetch_all(&st.db)
    .await?;
    let torrent_rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT t.release_id, t.platform, t.info_hash FROM torrents t
         JOIN releases r ON r.id = t.release_id WHERE r.game_id = ?",
    )
    .bind(game.id)
    .fetch_all(&st.db)
    .await?;
    let mut releases = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.get("id");
        let version: String = row.get("version");
        let platforms: String = row.get("platforms");
        let torrents: BTreeMap<String, TorrentRef> = torrent_rows
            .iter()
            .filter(|(release, _, _)| *release == id)
            .map(|(_, platform, info_hash)| {
                (
                    platform.clone(),
                    TorrentRef {
                        info_hash: info_hash.clone(),
                        url: format!(
                            "{}/games/{}/releases/{version}-{platform}.torrent",
                            st.cfg.public_url, game.slug
                        ),
                    },
                )
            })
            .collect();
        releases.push(ReleaseSummary {
            version,
            created: row.get("created"),
            platforms: platforms.split(',').map(str::to_string).collect(),
            torrents,
        });
    }
    Ok(ReleaseList {
        channels: db::channels(&st.db, game.id).await?,
        releases,
    })
}

pub async fn release_text(st: &AppState, game: &GameRow, version: &str) -> ApiResult<String> {
    sqlx::query_scalar("SELECT manifest FROM releases WHERE game_id = ? AND version = ?")
        .bind(game.id)
        .bind(version)
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("release {} {version}", game.slug)))
}

/// Store an uploaded chunk, enforcing hash and quota.
/// Returns `true` if it was new.
pub async fn store_chunk(
    st: &AppState,
    user: &User,
    id: ChunkId,
    data: bytes::Bytes,
) -> ApiResult<bool> {
    if data.is_empty() || data.len() as u64 > gamedist_core::MAX_CHUNK_SIZE {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "Bad chunk size",
            "chunks must be between 1 byte and 8 MiB",
        ));
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM chunks WHERE id = ?")
        .bind(id.to_hex())
        .fetch_optional(&st.db)
        .await?;
    if exists.is_some() && st.chunks.has(&id) {
        return Ok(false);
    }
    if let Some(quota) = db::quota_for(&st.db, user.id, st.cfg.default_quota_bytes).await? {
        let used = db::storage_used(&st.db, user.id).await?;
        if used + data.len() as u64 > quota {
            return Err(ApiError::new(
                axum::http::StatusCode::INSUFFICIENT_STORAGE,
                "Storage quota exceeded",
                format!("your account is limited to {quota} bytes ({used} used)"),
            ));
        }
    }
    let size = data.len() as i64;
    let store = st.chunks.clone();
    tokio::task::spawn_blocking(move || store.put(&id, &data))
        .await
        .map_err(|e| ApiError::internal("chunk write task", e))??;
    sqlx::query(
        "INSERT INTO chunks (id, size, uploader_id, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(id.to_hex())
    .bind(size)
    .bind(user.id)
    .bind(db::now())
    .execute(&st.db)
    .await?;
    Metrics::add(&st.metrics.chunks_uploaded, 1);
    Metrics::add(&st.metrics.chunk_bytes_uploaded, size as u64);
    Ok(true)
}

/// Take a game down (or restore it), recording the reason.
pub async fn takedown(
    st: &AppState,
    admin: &User,
    slug: &str,
    reason: &str,
    restore: bool,
) -> ApiResult<()> {
    validate_text("reason", reason, 2000, !restore)?;
    let game = db::game_by_slug(&st.db, slug)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("game {slug}")))?;
    sqlx::query("UPDATE games SET taken_down = ?, takedown_reason = ? WHERE id = ?")
        .bind(!restore)
        .bind(if restore { "" } else { reason })
        .bind(game.id)
        .execute(&st.db)
        .await?;
    let action = if restore { "restore" } else { "takedown" };
    sqlx::query(
        "INSERT INTO takedowns (slug, action, reason, actor_id, at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(slug)
    .bind(action)
    .bind(reason)
    .bind(admin.id)
    .bind(db::now())
    .execute(&st.db)
    .await?;
    st.evict_manifests(slug);
    db::audit(
        &st.db,
        admin.actor(),
        &format!("game.{action}"),
        slug,
        reason,
    )
    .await;
    Ok(())
}
