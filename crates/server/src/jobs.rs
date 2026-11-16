//! Background jobs: chunk garbage collection and session cleanup.

use crate::{db, state::St};
use gamedist_core::ChunkId;
use std::time::Duration;

/// Delete chunks that no release or game asset references and that are older
/// than the grace period. Returns `(chunks, bytes)` removed.
pub async fn collect_garbage(st: &St, grace_secs: i64) -> Result<(u64, u64), sqlx::Error> {
    let cutoff = db::now() - grace_secs;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT c.id, c.size FROM chunks c
         WHERE c.created_at <= ?
           AND NOT EXISTS (SELECT 1 FROM release_chunks rc WHERE rc.chunk_id = c.id)
           AND NOT EXISTS (SELECT 1 FROM game_assets ga WHERE ga.chunk_id = c.id)",
    )
    .bind(cutoff)
    .fetch_all(&st.db)
    .await?;
    let mut removed = (0u64, 0u64);
    for (id, size) in rows {
        // Re-check inside the delete so a release published meanwhile wins.
        let deleted = sqlx::query(
            "DELETE FROM chunks WHERE id = ?
               AND NOT EXISTS (SELECT 1 FROM release_chunks rc WHERE rc.chunk_id = chunks.id)
               AND NOT EXISTS (SELECT 1 FROM game_assets ga WHERE ga.chunk_id = chunks.id)",
        )
        .bind(&id)
        .execute(&st.db)
        .await?;
        if deleted.rows_affected() == 0 {
            continue;
        }
        if let Ok(chunk_id) = id.parse::<ChunkId>() {
            if let Err(e) = st.chunks.remove(&chunk_id) {
                tracing::warn!("gc: removing chunk {id}: {e}");
            }
        }
        removed.0 += 1;
        removed.1 += size as u64;
    }
    Ok(removed)
}

pub async fn purge_expired_sessions(st: &St) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM tokens WHERE expires_at IS NOT NULL AND expires_at < ?")
            .bind(db::now())
            .execute(&st.db)
            .await?
            .rows_affected(),
    )
}

/// Run maintenance hourly until the task is dropped.
pub async fn run(st: St) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
        let grace = st.cfg.gc_grace_days as i64 * 86_400;
        match collect_garbage(&st, grace).await {
            Ok((0, _)) => {}
            Ok((chunks, bytes)) => {
                tracing::info!("gc: removed {chunks} unreferenced chunks ({bytes} bytes)");
                db::audit(
                    &st.db,
                    None,
                    "gc.chunks",
                    &chunks.to_string(),
                    &format!("{bytes} bytes"),
                )
                .await;
            }
            Err(e) => tracing::error!("gc failed: {e}"),
        }
        if let Err(e) = purge_expired_sessions(&st).await {
            tracing::error!("session cleanup failed: {e}");
        }
    }
}
