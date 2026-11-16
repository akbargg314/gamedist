//! Persistent launcher state: settings, library entries with pinned keys,
//! catalogs, and the `installed.json` marker inside install directories.

use crate::{Error, Result};
use gamedist_core::{manifest::GameManifest, ChunkId, PublicKey};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub const LIBRARY_FILE: &str = "library.json";
pub const INSTALLED_MARKER: &str = "installed.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// HTTP chunk downloads only.
    #[default]
    Direct,
    /// BitTorrent with HTTP filling the gaps.
    TorrentDirect,
    /// BitTorrent only.
    Torrent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Games install to `<install_root>/<slug>` unless a directory is given.
    pub install_root: PathBuf,
    /// Download limit in bytes per second; `None` is unlimited.
    pub bandwidth_limit: Option<u64>,
    /// Parallel chunk downloads.
    pub concurrency: usize,
    /// Keep downloaded chunks after installing (faster re-installs and
    /// mirror sharing, at the cost of disk space).
    pub keep_chunks: bool,
    pub transport: Transport,
    /// Seed installed games to other players.
    pub seed: bool,
    /// Upload limit in bytes per second; `None` is unlimited.
    pub upload_limit: Option<u64>,
    /// Wine-compatible runner for Windows builds on other systems.
    pub wine: Option<PathBuf>,
    /// TCP port for incoming BitTorrent connections (0 = pick a free one).
    pub listen_port: u16,
    /// Give up on a torrent download after this many seconds without
    /// progress and (unless the transport is torrent-only) use HTTP.
    pub torrent_stall_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        let install_root = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gamedist")
            .join("games");
        Settings {
            install_root,
            bandwidth_limit: None,
            concurrency: 8,
            keep_chunks: false,
            transport: Transport::Direct,
            seed: false,
            upload_limit: None,
            wine: None,
            listen_port: 0,
            torrent_stall_secs: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub version: String,
    pub platform: String,
    pub dir: PathBuf,
    pub installed_at: String,
    /// Size of the installed files in bytes, from the signed manifest
    /// (0 in libraries written before this field existed, until backfilled).
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub game_id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description_md: String,
    /// Base URLs of every host this game was added from (mirrors).
    pub sources: Vec<String>,
    /// The developer key trusted for this game (TOFU pin).
    pub pinned_key: PublicKey,
    /// Channel this install follows.
    pub channel: String,
    /// Channels as last reported by a source (unsigned, informational).
    #[serde(default)]
    pub channels: BTreeMap<String, String>,
    #[serde(default)]
    pub icon: Option<ChunkId>,
    #[serde(default)]
    pub banner: Option<ChunkId>,
    #[serde(default)]
    pub screenshots: Vec<ChunkId>,
    /// URL prefix for asset images: `asset_base + chunk id`.
    #[serde(default)]
    pub asset_base: String,
    #[serde(default)]
    pub installed: Option<Installed>,
}

impl LibraryEntry {
    pub fn new(game: &GameManifest, source: &str) -> Self {
        let mut entry = LibraryEntry {
            game_id: game.game_id.clone(),
            slug: game.slug.clone(),
            title: String::new(),
            summary: String::new(),
            description_md: String::new(),
            sources: vec![source.to_string()],
            pinned_key: game.developer_pubkey,
            channel: if game.channels.contains_key("stable") || game.channels.is_empty() {
                "stable".to_string()
            } else {
                game.channels.keys().next().cloned().unwrap_or_default()
            },
            channels: BTreeMap::new(),
            icon: None,
            banner: None,
            screenshots: Vec::new(),
            asset_base: String::new(),
            installed: None,
        };
        entry.refresh_metadata(game);
        entry
    }

    /// Copy presentation metadata from a (key-checked) game manifest.
    /// Never touches the pin, the sources or the install record.
    pub fn refresh_metadata(&mut self, game: &GameManifest) {
        self.title = game.title.clone();
        self.summary = game.summary.clone();
        self.description_md = game.description_md.clone();
        self.channels = game.channels.clone();
        self.icon = game.assets.icon;
        self.banner = game.assets.banner;
        self.screenshots = game.assets.screenshots.clone();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub settings: Settings,
    /// Keyed by game ID.
    pub games: BTreeMap<String, LibraryEntry>,
    /// Base URLs of servers whose catalogs show up in Discover.
    pub catalogs: Vec<String>,
}

impl Library {
    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(LIBRARY_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                // Refuse to continue (and later overwrite pins) on a corrupt file.
                Error::msg(format!(
                    "{} is corrupt ({e}); fix or move it away. It holds your pinned developer keys.",
                    path.display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Library::default()),
            Err(e) => Err(Error::msg(format!("reading {}: {e}", path.display()))),
        }
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(gamedist_core::Error::from)?;
        write_atomic(&data_dir.join(LIBRARY_FILE), &text)
    }
}

/// Write via a temp file + rename so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let err = |e: std::io::Error| Error::msg(format!("writing {}: {e}", path.display()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, contents).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(e)
    })
}

/// `installed.json` inside the install directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledMarker {
    pub game_id: String,
    pub slug: String,
    pub version: String,
    pub platform: String,
    pub developer_pubkey: PublicKey,
    pub install_path: PathBuf,
}

pub fn write_installed_marker(entry: &LibraryEntry, installed: &Installed) -> Result<()> {
    let marker = InstalledMarker {
        game_id: entry.game_id.clone(),
        slug: entry.slug.clone(),
        version: installed.version.clone(),
        platform: installed.platform.clone(),
        developer_pubkey: entry.pinned_key,
        install_path: installed.dir.clone(),
    };
    let text = serde_json::to_string_pretty(&marker).map_err(gamedist_core::Error::from)?;
    write_atomic(&installed.dir.join(INSTALLED_MARKER), &text)
}

pub fn read_installed_marker(dir: &Path) -> Option<InstalledMarker> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(INSTALLED_MARKER)).ok()?).ok()
}

/// Refuse to install into a directory that belongs to another game, or into
/// a non-empty directory that is not a GameDist install at all (we would
/// otherwise overwrite unrelated files).
pub fn check_dir_free(dir: &Path, entry: &LibraryEntry) -> Result<()> {
    if let Some(marker) = read_installed_marker(dir) {
        // After a key rotation the game ID changes but slug and dir persist.
        let same_game = marker.game_id == entry.game_id
            || entry
                .installed
                .as_ref()
                .is_some_and(|i| i.dir == dir && marker.slug == entry.slug);
        return if same_game {
            Ok(())
        } else {
            Err(Error::msg(format!(
                "{} already contains another game ({})",
                dir.display(),
                marker.slug
            )))
        };
    }
    let occupied = std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if occupied && entry.installed.as_ref().is_none_or(|i| i.dir != dir) {
        return Err(Error::msg(format!(
            "{} is not empty; choose an empty directory",
            dir.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> LibraryEntry {
        let key = gamedist_core::SecretKey::generate().public();
        LibraryEntry {
            game_id: gamedist_core::game_id(&key, "cool-game"),
            slug: "cool-game".into(),
            title: "Cool".into(),
            summary: String::new(),
            description_md: String::new(),
            sources: vec!["https://host.example".into()],
            pinned_key: key,
            channel: "stable".into(),
            channels: BTreeMap::new(),
            icon: None,
            banner: None,
            screenshots: vec![],
            asset_base: String::new(),
            installed: None,
        }
    }

    #[test]
    fn library_roundtrip_and_corruption() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(Library::load(tmp.path()).unwrap().games.is_empty());
        let mut lib = Library::default();
        let e = entry();
        lib.games.insert(e.game_id.clone(), e.clone());
        lib.catalogs.push("https://host.example".into());
        lib.settings.transport = Transport::TorrentDirect;
        lib.save(tmp.path()).unwrap();
        let loaded = Library::load(tmp.path()).unwrap();
        assert_eq!(loaded.games[&e.game_id], e);
        assert_eq!(loaded.settings.transport, Transport::TorrentDirect);
        let text = std::fs::read_to_string(tmp.path().join(LIBRARY_FILE)).unwrap();
        assert!(text.contains("\"torrent-direct\""));

        std::fs::write(tmp.path().join(LIBRARY_FILE), "{ truncated").unwrap();
        let err = Library::load(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("corrupt"), "{err}");
    }

    #[test]
    fn install_dir_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mine = entry();
        let dir = tmp.path().join("game");
        // Missing or empty directory is fine.
        check_dir_free(&dir, &mine).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        check_dir_free(&dir, &mine).unwrap();
        // Unrelated files are not.
        std::fs::write(dir.join("thesis.docx"), b"x").unwrap();
        assert!(check_dir_free(&dir, &mine).is_err());
        // Our own install is.
        let installed = Installed {
            version: "1.0.0".into(),
            platform: "linux-x86_64".into(),
            dir: dir.clone(),
            installed_at: String::new(),
            size: 0,
        };
        mine.installed = Some(installed.clone());
        write_installed_marker(&mine, &installed).unwrap();
        check_dir_free(&dir, &mine).unwrap();
        assert_eq!(read_installed_marker(&dir).unwrap().version, "1.0.0");
        // Someone else's is not.
        let mut other = entry();
        other.slug = "other".into();
        assert!(check_dir_free(&dir, &other).is_err());
    }
}
