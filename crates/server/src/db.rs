//! SQLite schema, migrations and small shared queries.

use crate::error::ApiError;
use gamedist_core::manifest::Registration;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::path::Path;

/// Each entry is one migration; `PRAGMA user_version` records how many ran.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    disabled      INTEGER NOT NULL DEFAULT 0,
    quota_bytes   INTEGER,              -- NULL: server default; 0: unlimited
    created_at    INTEGER NOT NULL
);
CREATE TABLE tokens (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    hash         TEXT NOT NULL UNIQUE,  -- sha256 of the token; the token itself is never stored
    kind         TEXT NOT NULL,         -- 'api' | 'session'
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at   INTEGER
);
CREATE TABLE invites (
    code       TEXT PRIMARY KEY,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    used_by    INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE dev_keys (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pubkey     TEXT NOT NULL,
    label      TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    UNIQUE (user_id, pubkey)
);
CREATE TABLE games (
    id               INTEGER PRIMARY KEY,
    slug             TEXT NOT NULL UNIQUE,
    game_id          TEXT NOT NULL,
    owner_id         INTEGER NOT NULL REFERENCES users(id),
    developer_pubkey TEXT NOT NULL,
    title            TEXT NOT NULL,
    summary          TEXT NOT NULL DEFAULT '',
    description_md   TEXT NOT NULL DEFAULT '',
    assets_json      TEXT NOT NULL DEFAULT '{}',
    rotations_json   TEXT NOT NULL DEFAULT '[]',
    taken_down       INTEGER NOT NULL DEFAULT 0,
    takedown_reason  TEXT NOT NULL DEFAULT '',
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);
CREATE TABLE game_assets (
    game_id  INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    chunk_id TEXT NOT NULL,
    PRIMARY KEY (game_id, chunk_id)
);
CREATE INDEX game_assets_chunk ON game_assets(chunk_id);
CREATE TABLE releases (
    id         INTEGER PRIMARY KEY,
    game_id    INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    version    TEXT NOT NULL,
    manifest   TEXT NOT NULL,           -- exactly as signed and submitted
    created    TEXT NOT NULL,           -- the manifest's own timestamp
    platforms  TEXT NOT NULL,           -- comma separated
    total_size INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (game_id, version)
);
CREATE TABLE release_chunks (
    release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    chunk_id   TEXT NOT NULL,
    PRIMARY KEY (release_id, chunk_id)
) WITHOUT ROWID;
CREATE INDEX release_chunks_chunk ON release_chunks(chunk_id);
CREATE TABLE channels (
    game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    name    TEXT NOT NULL,
    version TEXT NOT NULL,
    PRIMARY KEY (game_id, name)
);
CREATE TABLE torrents (
    release_id INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    platform   TEXT NOT NULL,
    info_hash  TEXT NOT NULL,
    data       BLOB NOT NULL,
    PRIMARY KEY (release_id, platform)
);
CREATE INDEX torrents_info_hash ON torrents(info_hash);
CREATE TABLE chunks (
    id          TEXT PRIMARY KEY,
    size        INTEGER NOT NULL,
    uploader_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX chunks_uploader ON chunks(uploader_id);
CREATE TABLE takedowns (
    id       INTEGER PRIMARY KEY,
    slug     TEXT NOT NULL,
    action   TEXT NOT NULL,             -- 'takedown' | 'restore'
    reason   TEXT NOT NULL,
    actor_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    at       INTEGER NOT NULL
);
CREATE TABLE audit_log (
    id       INTEGER PRIMARY KEY,
    at       INTEGER NOT NULL,
    actor_id INTEGER,
    actor    TEXT NOT NULL,
    action   TEXT NOT NULL,
    target   TEXT NOT NULL,
    detail   TEXT NOT NULL DEFAULT ''
);
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
"#];

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub async fn open(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::raw_sql(&format!("PRAGMA user_version = {}", index + 1))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

pub async fn get_setting(db: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await
}

pub async fn set_setting(db: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn registration_mode(
    db: &SqlitePool,
    default: Registration,
) -> Result<Registration, sqlx::Error> {
    Ok(get_setting(db, "registration")
        .await?
        .and_then(|v| Registration::parse(&v).ok())
        .unwrap_or(default))
}

/// Server-wide default quota in bytes (0 = unlimited).
pub async fn default_quota(db: &SqlitePool, default: u64) -> Result<u64, sqlx::Error> {
    Ok(get_setting(db, "default_quota_bytes")
        .await?
        .and_then(|v| v.parse().ok())
        .unwrap_or(default))
}

pub async fn audit(
    db: &SqlitePool,
    actor: Option<(i64, &str)>,
    action: &str,
    target: &str,
    detail: &str,
) {
    let result = sqlx::query(
        "INSERT INTO audit_log (at, actor_id, actor, action, target, detail) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(now())
    .bind(actor.map(|a| a.0))
    .bind(actor.map(|a| a.1).unwrap_or("system"))
    .bind(action)
    .bind(target)
    .bind(detail)
    .execute(db)
    .await;
    if let Err(e) = result {
        tracing::error!("writing audit log: {e}");
    }
}

/// A row of `games`.
#[derive(Debug, Clone)]
pub struct GameRow {
    pub id: i64,
    pub slug: String,
    pub game_id: String,
    pub owner_id: i64,
    pub developer_pubkey: String,
    pub title: String,
    pub summary: String,
    pub description_md: String,
    pub assets_json: String,
    pub rotations_json: String,
    pub taken_down: bool,
    pub takedown_reason: String,
}

impl GameRow {
    pub fn from_row(row: &sqlx::sqlite::SqliteRow) -> Self {
        GameRow {
            id: row.get("id"),
            slug: row.get("slug"),
            game_id: row.get("game_id"),
            owner_id: row.get("owner_id"),
            developer_pubkey: row.get("developer_pubkey"),
            title: row.get("title"),
            summary: row.get("summary"),
            description_md: row.get("description_md"),
            assets_json: row.get("assets_json"),
            rotations_json: row.get("rotations_json"),
            taken_down: row.get::<i64, _>("taken_down") != 0,
            takedown_reason: row.get("takedown_reason"),
        }
    }
}

/// Look a game up by slug, including taken-down ones.
pub async fn game_by_slug(db: &SqlitePool, slug: &str) -> Result<Option<GameRow>, ApiError> {
    let row = sqlx::query("SELECT * FROM games WHERE slug = ?")
        .bind(slug)
        .fetch_optional(db)
        .await?;
    Ok(row.as_ref().map(GameRow::from_row))
}

pub async fn channels(
    db: &SqlitePool,
    game: i64,
) -> Result<std::collections::BTreeMap<String, String>, sqlx::Error> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT name, version FROM channels WHERE game_id = ?")
            .bind(game)
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Bytes of chunks this account uploaded.
pub async fn storage_used(db: &SqlitePool, user: i64) -> Result<u64, sqlx::Error> {
    let used: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size), 0) FROM chunks WHERE uploader_id = ?")
            .bind(user)
            .fetch_one(db)
            .await?;
    Ok(used as u64)
}

/// Effective quota for a user: `None` = unlimited.
pub async fn quota_for(
    db: &SqlitePool,
    user: i64,
    config_default: u64,
) -> Result<Option<u64>, sqlx::Error> {
    let own: Option<Option<i64>> = sqlx::query_scalar("SELECT quota_bytes FROM users WHERE id = ?")
        .bind(user)
        .fetch_optional(db)
        .await?;
    let quota = match own.flatten() {
        Some(q) => q as u64,
        None => default_quota(db, config_default).await?,
    };
    Ok((quota != 0).then_some(quota))
}
