//! Torrent generation for published releases.

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use gamedist_core::{
    manifest::ReleaseManifest,
    torrent::{build_metainfo, piece_length_for, PieceHasher, TorrentFile},
};

/// Name of every generated torrent; see SPEC §4.5 for why it is fixed.
pub const TORRENT_NAME: &str = "files";

/// Build and store one torrent per platform of a release.
pub async fn generate(st: &AppState, release_id: i64, manifest: &ReleaseManifest) -> ApiResult<()> {
    let mut trackers: Vec<String> = st.cfg.tracker_url().into_iter().collect();
    trackers.extend(st.cfg.external_trackers.iter().cloned());

    for (platform_name, platform) in &manifest.platforms {
        if platform.total_size == 0 {
            continue;
        }
        let piece_length = piece_length_for(platform.total_size);
        let store = st.chunks.clone();
        let files = platform.files.clone();
        let pieces = tokio::task::spawn_blocking(move || -> gamedist_core::Result<Vec<u8>> {
            let mut hasher = PieceHasher::new(piece_length);
            for file in &files {
                for chunk in &file.chunks {
                    hasher.update(&store.get_unverified(&chunk.id)?);
                }
            }
            Ok(hasher.finish())
        })
        .await
        .map_err(|e| ApiError::internal("torrent hashing task", e))??;

        let files: Vec<TorrentFile> = platform
            .files
            .iter()
            .map(|f| TorrentFile {
                path: f.path.clone(),
                size: f.size,
            })
            .collect();
        let web_seed = format!(
            "{}/games/{}/releases/{}/{}/",
            st.cfg.public_url, manifest.slug, manifest.version, platform_name
        );
        let metainfo = build_metainfo(
            TORRENT_NAME,
            &files,
            piece_length,
            pieces,
            &trackers,
            &[web_seed],
            &format!("{} {} ({platform_name})", manifest.slug, manifest.version),
        )?;
        sqlx::query(
            "INSERT OR REPLACE INTO torrents (release_id, platform, info_hash, data) VALUES (?, ?, ?, ?)",
        )
        .bind(release_id)
        .bind(platform_name)
        .bind(metainfo.info_hash_hex())
        .bind(metainfo.bytes)
        .execute(&st.db)
        .await?;
    }
    Ok(())
}
