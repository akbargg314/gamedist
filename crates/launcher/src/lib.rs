//! GameDist launcher engine: library, trust-on-first-use key pinning, install,
//! delta update, verify/repair, launch and catalogs.
//!
//! This crate has no UI. The Tauri app (`apps/launcher`) and the headless CLI
//! ([`headless`]) are thin layers over [`Launcher`].

pub mod headless;
pub mod library;
pub mod torrent;

use gamedist_core::{
    api::GameSummary,
    client::{http_client, ApiClient},
    download::{fetch_chunks, DownloadOptions},
    ids::{self, GameUrl},
    install,
    manifest::{rotation_chain_links, GameManifest, Platform, ReleaseManifest, SignedRelease},
    progress::{Phase, Progress, ProgressFn},
    store::ContentStore,
};
pub use library::{Installed, Library, LibraryEntry, Settings, Transport};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Core(#[from] gamedist_core::Error),

    /// A source presents a developer key other than the pinned one and no
    /// valid rotation statement links them.
    #[error(
        "DEVELOPER KEY CHANGED for {slug}: this game is pinned to key {pinned} but {source_url} now presents {presented}. \
         The server may be compromised or impersonating the game. Nothing was changed. \
         Only if the developer announced a new key through a channel you trust, retry and explicitly accept the key change."
    )]
    KeyChanged {
        slug: String,
        source_url: String,
        pinned: String,
        presented: String,
    },

    #[error("{0}")]
    Message(String),
}

impl Error {
    pub fn msg(message: impl Into<String>) -> Self {
        Error::Message(message.into())
    }
}

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Install directory; defaults to `<install_root>/<slug>`, or the
    /// directory of the existing install.
    pub dir: Option<PathBuf>,
    /// Defaults to this machine's platform.
    pub platform: Option<String>,
    /// Switch the entry to this channel.
    pub channel: Option<String>,
    /// Install exactly this version instead of the channel's.
    pub version: Option<String>,
    /// Permit moving to a lower version than the installed one.
    pub allow_downgrade: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallReport {
    pub game_id: String,
    pub version: String,
    pub previous_version: Option<String>,
    pub platform: String,
    pub dir: PathBuf,
    /// Chunks/bytes fetched over the network.
    pub downloaded_chunks: u64,
    pub downloaded_bytes: u64,
    /// Bytes recovered from the previously installed files.
    pub harvested_bytes: u64,
    /// Bytes obtained (and verified) from the BitTorrent swarm.
    pub torrent_bytes: u64,
    pub files_written: usize,
    pub files_deleted: usize,
    /// `true` when the install was already at this version and intact.
    pub up_to_date: bool,
    /// The install is now being seeded to other players.
    pub seeding: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateInfo {
    pub game_id: String,
    pub installed: Option<String>,
    pub latest: String,
    pub update_available: bool,
    /// Bytes that would have to be downloaded.
    pub download_bytes: u64,
    /// Full size of the new version for this platform.
    pub total_bytes: u64,
    pub changelog_md: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyReport {
    pub game_id: String,
    pub checked_files: usize,
    pub bad_files: Vec<String>,
    pub repaired: bool,
    pub downloaded_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoverEntry {
    pub server: String,
    pub server_name: String,
    /// Already in the local library.
    pub in_library: bool,
    /// URL prefix for the game's images: `asset_base + chunk id`.
    pub asset_base: String,
    #[serde(flatten)]
    pub game: GameSummary,
}

/// A game resolved against one of its sources.
struct Resolved {
    client: ApiClient,
    game: GameManifest,
}

pub struct Launcher {
    data_dir: PathBuf,
    store: ContentStore,
    http: reqwest::Client,
    library: Mutex<Library>,
    /// One async lock per game: operations that read or change a game's
    /// installed files (install, verify, uninstall, seed) never overlap.
    ops: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    #[cfg(feature = "torrent")]
    engine: tokio::sync::OnceCell<torrent::TorrentEngine>,
}

impl Launcher {
    /// Open (or create) launcher state in `data_dir`.
    pub fn open(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| Error::msg(format!("creating {}: {e}", data_dir.display())))?;
        let library = Library::load(&data_dir)?;
        let launcher = Launcher {
            store: ContentStore::open(data_dir.join("store"))?,
            http: http_client()?,
            library: Mutex::new(library),
            ops: Mutex::new(HashMap::new()),
            #[cfg(feature = "torrent")]
            engine: tokio::sync::OnceCell::new(),
            data_dir,
        };
        launcher.backfill_installed_sizes();
        Ok(launcher)
    }

    /// Libraries written before `Installed::size` existed have 0 there; fill
    /// it in from the stored manifests. Best effort: a missing manifest
    /// leaves the size at 0.
    fn backfill_installed_sizes(&self) {
        let stale: Vec<LibraryEntry> = self
            .lib()
            .games
            .values()
            .filter(|g| g.installed.as_ref().is_some_and(|i| i.size == 0))
            .cloned()
            .collect();
        let mut changed = false;
        for entry in stale {
            let Some(platform) = self.installed_platform(&entry) else {
                continue;
            };
            if platform.total_size == 0 {
                continue;
            }
            if let Some(installed) = self
                .lib()
                .games
                .get_mut(&entry.game_id)
                .and_then(|g| g.installed.as_mut())
            {
                installed.size = platform.total_size;
                changed = true;
            }
        }
        if changed {
            let _ = self.save();
        }
    }

    /// Per-user default data directory (`$GAMEDIST_DATA_DIR` overrides).
    pub fn default_data_dir() -> Result<PathBuf> {
        if let Some(dir) = std::env::var_os("GAMEDIST_DATA_DIR") {
            return Ok(PathBuf::from(dir));
        }
        dirs::data_dir()
            .map(|d| d.join("gamedist"))
            .ok_or_else(|| Error::msg("cannot determine the data directory; set GAMEDIST_DATA_DIR"))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn store(&self) -> &ContentStore {
        &self.store
    }

    fn lib(&self) -> std::sync::MutexGuard<'_, Library> {
        self.library.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The lock that serializes file operations on one game.
    fn op_lock(&self, game_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.ops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(game_id.to_string())
            .or_default()
            .clone()
    }

    fn save(&self) -> Result<()> {
        // Clone under the lock, write outside it.
        let snapshot = self.lib().clone();
        snapshot.save(&self.data_dir)
    }

    fn put_entry(&self, entry: &LibraryEntry) -> Result<()> {
        self.lib()
            .games
            .insert(entry.game_id.clone(), entry.clone());
        self.save()
    }

    pub fn settings(&self) -> Settings {
        self.lib().settings.clone()
    }

    pub fn update_settings(&self, edit: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let settings = {
            let mut lib = self.lib();
            edit(&mut lib.settings);
            lib.settings.clone()
        };
        self.save()?;
        Ok(settings)
    }

    pub fn games(&self) -> Vec<LibraryEntry> {
        self.lib().games.values().cloned().collect()
    }

    /// Find a library entry by game ID, slug or game URL.
    pub fn find(&self, selector: &str) -> Result<LibraryEntry> {
        let lib = self.lib();
        if let Some(entry) = lib.games.get(selector) {
            return Ok(entry.clone());
        }
        let (slug, base) = match GameUrl::parse(selector) {
            Ok(url) => (url.slug, Some(url.base)),
            Err(_) => (selector.to_string(), None),
        };
        let mut matches = lib
            .games
            .values()
            .filter(|g| g.slug == slug && base.as_ref().is_none_or(|b| g.sources.contains(b)));
        match (matches.next(), matches.next()) {
            (Some(entry), None) => Ok(entry.clone()),
            (Some(_), Some(_)) => Err(Error::msg(format!(
                "several games in the library are called {slug:?}; use the game ID or full URL"
            ))),
            _ => Err(Error::msg(format!("{selector:?} is not in the library"))),
        }
    }

    fn manifest_path(&self, game_id: &str, version: &str) -> PathBuf {
        self.data_dir
            .join("manifests")
            .join(game_id)
            .join(format!("{version}.json"))
    }

    /// The signed manifest of an installed version. It was verified against
    /// the pinned key before it was stored; on load it is re-checked for
    /// integrity (schema, structure, signature by its embedded key).
    fn load_manifest(&self, game_id: &str, version: &str) -> Result<ReleaseManifest> {
        let path = self.manifest_path(game_id, version);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::msg(format!("reading {}: {e}", path.display())))?;
        Ok(SignedRelease::parse(&text)?.into_manifest())
    }

    fn installed_platform(&self, entry: &LibraryEntry) -> Option<Platform> {
        let installed = entry.installed.as_ref()?;
        self.load_manifest(&entry.game_id, &installed.version)
            .ok()?
            .platforms
            .remove(&installed.platform)
    }

    // ---- adding games ------------------------------------------------------

    /// Add a game by URL: fetch its manifest, pin the developer key on first
    /// sight (TOFU) and remember the source. Adding the same game from a
    /// second host adds a mirror to the existing entry.
    pub async fn add_game(&self, url: &str, accept_key_change: bool) -> Result<LibraryEntry> {
        let url = GameUrl::parse(url)?;
        let client = ApiClient::connect_with(self.http.clone(), &url.base).await?;
        // `ApiClient::game` checks that game_id == f(developer_pubkey, slug).
        let game = client.game(&url.slug).await?;

        let same_id = self.lib().games.get(&game.game_id).cloned();
        if let Some(mut entry) = same_id {
            // Same game ID implies the same key and slug by construction.
            if !entry.sources.contains(&url.base) {
                entry.sources.push(url.base.clone());
            }
            entry.refresh_metadata(&game);
            entry.asset_base = client.info().chunks.clone();
            self.put_entry(&entry)?;
            return Ok(entry);
        }

        // Same slug from a host we already use, but under a different key?
        let conflicting = self
            .lib()
            .games
            .values()
            .find(|g| g.slug == url.slug && g.sources.contains(&url.base))
            .cloned();
        if let Some(old) = conflicting {
            return self.handle_key_change(old, &client, &game, accept_key_change);
        }

        let mut entry = LibraryEntry::new(&game, &url.base);
        entry.asset_base = client.info().chunks.clone();
        self.put_entry(&entry)?;
        Ok(entry)
    }

    /// A source presents a different developer key for a pinned game. Follow
    /// a valid rotation chain, or an explicit user decision; refuse otherwise.
    fn handle_key_change(
        &self,
        old: LibraryEntry,
        client: &ApiClient,
        game: &GameManifest,
        accept: bool,
    ) -> Result<LibraryEntry> {
        let rotated = rotation_chain_links(
            &old.slug,
            &old.pinned_key,
            &game.developer_pubkey,
            &game.rotations,
        );
        if !rotated && !accept {
            return Err(Error::KeyChanged {
                slug: old.slug.clone(),
                source_url: client.base().to_string(),
                pinned: old.pinned_key.to_string(),
                presented: game.developer_pubkey.to_string(),
            });
        }
        // Re-key the entry: the game ID is derived from the key. Other
        // mirrors are dropped; they are re-added when they present the new key.
        let mut entry = old.clone();
        entry.game_id = game.game_id.clone();
        entry.pinned_key = game.developer_pubkey;
        entry.sources = vec![client.base().to_string()];
        entry.refresh_metadata(game);
        entry.asset_base = client.info().chunks.clone();
        if let Some(installed) = &entry.installed {
            // Carry the stored manifest over so the next update is a delta.
            let from = self.manifest_path(&old.game_id, &installed.version);
            let to = self.manifest_path(&entry.game_id, &installed.version);
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(from, to);
        }
        {
            let mut lib = self.lib();
            lib.games.remove(&old.game_id);
            lib.games.insert(entry.game_id.clone(), entry.clone());
        }
        self.save()?;
        if let Some(installed) = &entry.installed {
            library::write_installed_marker(&entry, installed)?;
        }
        let _ = std::fs::remove_dir_all(self.data_dir.join("manifests").join(&old.game_id));
        Ok(entry)
    }

    /// Contact the game's sources in order until one answers. A source that
    /// presents a different key either carries a valid rotation chain (the
    /// entry is re-keyed) or the whole operation stops with `KeyChanged`.
    async fn resolve(&self, entry: &LibraryEntry) -> Result<(Resolved, LibraryEntry)> {
        let mut last_error = Error::msg(format!("{} has no download source", entry.slug));
        for source in &entry.sources {
            let attempt = async {
                let client = ApiClient::connect_with(self.http.clone(), source).await?;
                let game = client.game(&entry.slug).await?;
                Ok::<_, Error>(Resolved { client, game })
            };
            match attempt.await {
                Ok(resolved) => {
                    let entry = if resolved.game.developer_pubkey != entry.pinned_key {
                        self.handle_key_change(
                            entry.clone(),
                            &resolved.client,
                            &resolved.game,
                            false,
                        )?
                    } else {
                        let mut updated = entry.clone();
                        updated.refresh_metadata(&resolved.game);
                        updated.asset_base = resolved.client.info().chunks.clone();
                        if updated != *entry {
                            self.put_entry(&updated)?;
                        }
                        updated
                    };
                    return Ok((resolved, entry));
                }
                Err(e) => last_error = e,
            }
        }
        Err(last_error)
    }

    fn pick_platform(&self, manifest: &ReleaseManifest, wanted: Option<&str>) -> Result<String> {
        let available = || {
            manifest
                .platforms
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(wanted) = wanted {
            return if manifest.platforms.contains_key(wanted) {
                Ok(wanted.to_string())
            } else {
                Err(Error::msg(format!(
                    "{} {} has no {wanted} build (available: {})",
                    manifest.slug,
                    manifest.version,
                    available()
                )))
            };
        }
        let host = ids::host_platform();
        if manifest.platforms.contains_key(&host) {
            return Ok(host);
        }
        // Linux and macOS can run Windows builds through a Wine-compatible runner.
        let windows = format!("windows-{}", std::env::consts::ARCH);
        if !cfg!(target_os = "windows")
            && manifest.platforms.contains_key(&windows)
            && self.find_wine().is_some()
        {
            return Ok(windows);
        }
        Err(Error::msg(format!(
            "{} {} has no build for {host} (available: {})",
            manifest.slug,
            manifest.version,
            available()
        )))
    }

    /// Fetch and verify the release an install/update would move to.
    async fn target_release(
        &self,
        entry: &LibraryEntry,
        resolved: &Resolved,
        opts: &InstallOptions,
    ) -> Result<SignedRelease> {
        let channel = opts.channel.as_deref().unwrap_or(&entry.channel);
        let version = match &opts.version {
            Some(v) => v.clone(),
            None => resolved
                .game
                .channels
                .get(channel)
                .cloned()
                .ok_or_else(|| {
                    Error::msg(format!(
                        "{} has no release on the {channel:?} channel (channels: {})",
                        entry.slug,
                        resolved
                            .game
                            .channels
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?,
        };
        // The channel pointer is unsigned: a server could point it at an old,
        // vulnerable but validly signed release.
        if let (Some(installed), false) = (&entry.installed, opts.allow_downgrade) {
            let current = ids::parse_version(&installed.version)?;
            let target = ids::parse_version(&version)?;
            if target < current {
                return Err(Error::msg(format!(
                    "refusing to downgrade {} from {} to {version}: the server points at an older release. \
                     Allow the downgrade explicitly if this is intended.",
                    entry.slug, installed.version
                )));
            }
        }
        // Verifies schema, structure, signature against the *pinned* key, and
        // that slug and version are the ones asked for.
        let release = resolved
            .client
            .release(&entry.slug, &version, &entry.pinned_key)
            .await?;
        if release.manifest().game_id != entry.game_id {
            return Err(Error::msg("release belongs to a different game ID"));
        }
        Ok(release)
    }

    /// Chunk URL prefixes of every reachable source, primary first.
    async fn chunk_bases(&self, entry: &LibraryEntry, primary: &ApiClient) -> Vec<String> {
        let mut bases = vec![primary.info().chunks.clone()];
        for source in &entry.sources {
            if source == primary.base() {
                continue;
            }
            if let Ok(client) = ApiClient::connect_with(self.http.clone(), source).await {
                bases.push(client.info().chunks.clone());
            }
        }
        bases
    }

    fn download_options(&self) -> DownloadOptions {
        let settings = self.settings();
        DownloadOptions {
            concurrency: settings.concurrency.max(1),
            rate_limit: settings.bandwidth_limit.filter(|l| *l > 0),
            ..Default::default()
        }
    }

    // ---- install / update --------------------------------------------------

    /// What an update would involve, without changing anything on disk.
    pub async fn check_update(&self, selector: &str) -> Result<UpdateInfo> {
        let entry = self.find(selector)?;
        let (resolved, entry) = self.resolve(&entry).await?;
        let release = self
            .target_release(&entry, &resolved, &InstallOptions::default())
            .await?;
        let manifest = release.manifest();
        let wanted = entry.installed.as_ref().map(|i| i.platform.as_str());
        let platform_name = self.pick_platform(manifest, wanted)?;
        let platform = &manifest.platforms[&platform_name];
        let old = self.installed_platform(&entry);
        let dir = entry
            .installed
            .as_ref()
            .map(|i| i.dir.clone())
            .unwrap_or_else(|| self.settings().install_root.join(&entry.slug));
        let plan = install::plan_install(platform, old.as_ref(), &dir);
        let have: HashSet<_> = old
            .iter()
            .flat_map(|p| p.files.iter())
            .flat_map(|f| f.chunks.iter())
            .map(|c| c.id)
            .collect();
        let download_bytes = plan
            .chunks
            .iter()
            .filter(|c| !self.store.has(&c.id) && !have.contains(&c.id))
            .map(|c| c.size)
            .sum();
        let installed = entry.installed.as_ref().map(|i| i.version.clone());
        Ok(UpdateInfo {
            game_id: entry.game_id.clone(),
            update_available: installed.as_deref() != Some(manifest.version.as_str()),
            installed,
            latest: manifest.version.clone(),
            download_bytes,
            total_bytes: platform.total_size,
            changelog_md: manifest.changelog_md.clone(),
        })
    }

    /// Install a game, or update an existing install. Only chunks that are
    /// neither in the store nor recoverable from the installed files are
    /// downloaded; every chunk is verified before use; files are swapped in
    /// atomically with rollback on failure.
    pub async fn install(
        &self,
        selector: &str,
        opts: InstallOptions,
        progress: ProgressFn,
    ) -> Result<InstallReport> {
        let game_id = self.find(selector)?.game_id;
        let lock = self.op_lock(&game_id);
        let _guard = lock.lock().await;
        self.install_locked(&game_id, opts, progress).await
    }

    async fn install_locked(
        &self,
        selector: &str,
        opts: InstallOptions,
        progress: ProgressFn,
    ) -> Result<InstallReport> {
        let entry = self.find(selector)?;
        let (resolved, mut entry) = self.resolve(&entry).await?;
        if let Some(channel) = &opts.channel {
            ids::validate_channel(channel)?;
            entry.channel = channel.clone();
        }
        let release = self.target_release(&entry, &resolved, &opts).await?;
        let manifest = release.manifest().clone();
        let previous = entry.installed.clone();
        let wanted_platform = opts
            .platform
            .clone()
            .or_else(|| previous.as_ref().map(|i| i.platform.clone()));
        let platform_name = self.pick_platform(&manifest, wanted_platform.as_deref())?;
        let platform = manifest.platforms[&platform_name].clone();

        let dir = match (&opts.dir, &previous) {
            (Some(dir), _) => dir.clone(),
            (None, Some(prev)) => prev.dir.clone(),
            (None, None) => self.settings().install_root.join(&entry.slug),
        };
        let dir =
            std::path::absolute(&dir).map_err(|e| Error::msg(format!("{}: {e}", dir.display())))?;
        if let Some(prev) = &previous {
            if prev.dir != dir {
                return Err(Error::msg(format!(
                    "{} is already installed in {}; uninstall it first to move it",
                    entry.slug,
                    prev.dir.display()
                )));
            }
        }
        library::check_dir_free(&dir, &entry)?;
        // The files are about to change under the torrent client's feet.
        self.stop_seeding(&entry.game_id).await;

        // The old tree is only reusable if it is the same platform.
        let old = previous
            .as_ref()
            .filter(|p| p.platform == platform_name)
            .and_then(|_| self.installed_platform(&entry));

        let (plan, harvested) = {
            let (platform, old, dir, store) = (
                platform.clone(),
                old.clone(),
                dir.clone(),
                self.store.clone(),
            );
            let progress = progress.clone();
            tokio::task::spawn_blocking(move || -> Result<_> {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| Error::msg(format!("creating {}: {e}", dir.display())))?;
                install::recover_interrupted(&dir)?;
                let plan = install::plan_install(&platform, old.as_ref(), &dir);
                let mut harvested = 0;
                if let Some(old) = &old {
                    harvested =
                        install::harvest_with(old, &dir, &plan.chunks, &store, |done, total| {
                            progress(Progress::local(Phase::Harvest, done, total));
                        })?;
                }
                Ok((plan, harvested))
            })
            .await
            .map_err(|e| Error::msg(format!("planning task failed: {e}")))??
        };
        let up_to_date = plan.is_noop()
            && previous
                .as_ref()
                .is_some_and(|p| p.version == manifest.version);

        let mut missing = install::missing_chunks(&plan, &self.store);
        let transport = self.settings().transport;
        let mut torrent_bytes = 0;
        if !missing.is_empty() && transport != Transport::Direct {
            let fetched = self
                .torrent_fetch(
                    &resolved.client,
                    &manifest,
                    &platform_name,
                    &plan,
                    &missing,
                    &progress,
                )
                .await;
            match fetched {
                Ok(bytes) => torrent_bytes = bytes,
                Err(e) if transport == Transport::Torrent => return Err(e),
                // torrent+direct: HTTP covers whatever the swarm did not.
                Err(_) => {}
            }
            missing = install::missing_chunks(&plan, &self.store);
            if !missing.is_empty() && transport == Transport::Torrent {
                return Err(Error::msg(format!(
                    "the torrent swarm could not provide {} chunk(s) and the transport is set to torrent only",
                    missing.len()
                )));
            }
        }
        let mut stats = Default::default();
        if !missing.is_empty() {
            let bases = self.chunk_bases(&entry, &resolved.client).await;
            stats = fetch_chunks(
                &self.http,
                &bases,
                &self.store,
                &missing,
                &self.download_options(),
                progress.clone(),
            )
            .await?;
        }

        {
            let (platform, dir, store, plan) = (
                platform.clone(),
                dir.clone(),
                self.store.clone(),
                plan.clone(),
            );
            let progress = progress.clone();
            tokio::task::spawn_blocking(move || {
                let total = plan.write_bytes(&platform);
                let mut done = 0;
                install::apply_install(&platform, &dir, &store, &plan, |n| {
                    done += n;
                    progress(Progress::local(Phase::Assemble, done, total));
                })
            })
            .await
            .map_err(|e| Error::msg(format!("install task failed: {e}")))??;
        }

        // Record the new state only after the files are in place.
        library::write_atomic(
            &self.manifest_path(&entry.game_id, &manifest.version),
            release.text(),
        )?;
        let installed = Installed {
            version: manifest.version.clone(),
            platform: platform_name.clone(),
            dir: dir.clone(),
            installed_at: gamedist_core::now_rfc3339(),
            size: platform.total_size,
        };
        entry.installed = Some(installed.clone());
        library::write_installed_marker(&entry, &installed)?;
        self.put_entry(&entry)?;
        if let Some(prev) = &previous {
            if prev.version != manifest.version {
                let _ = std::fs::remove_file(self.manifest_path(&entry.game_id, &prev.version));
            }
        }
        if !self.settings().keep_chunks {
            for chunk in &plan.chunks {
                let _ = self.store.remove(&chunk.id);
            }
        }
        let seeding = self.settings().seed && self.seed_game_locked(&entry.game_id).await.is_ok();
        progress(Progress::local(
            Phase::Done,
            platform.total_size,
            platform.total_size,
        ));
        Ok(InstallReport {
            game_id: entry.game_id.clone(),
            version: manifest.version.clone(),
            previous_version: previous.map(|p| p.version),
            platform: platform_name,
            dir,
            downloaded_chunks: stats.chunks,
            downloaded_bytes: stats.bytes,
            harvested_bytes: harvested,
            torrent_bytes,
            files_written: plan.write.len(),
            files_deleted: plan.delete.len(),
            up_to_date,
            seeding,
        })
    }

    // ---- verify / repair ---------------------------------------------------

    /// Re-hash the installed files; with `repair`, re-download what is damaged.
    pub async fn verify(
        &self,
        selector: &str,
        repair: bool,
        progress: ProgressFn,
    ) -> Result<VerifyReport> {
        let game_id = self.find(selector)?.game_id;
        let lock = self.op_lock(&game_id);
        let _guard = lock.lock().await;
        self.verify_locked(&game_id, repair, progress).await
    }

    async fn verify_locked(
        &self,
        selector: &str,
        repair: bool,
        progress: ProgressFn,
    ) -> Result<VerifyReport> {
        let entry = self.find(selector)?;
        let installed = entry
            .installed
            .clone()
            .ok_or_else(|| Error::msg(format!("{} is not installed", entry.slug)))?;
        let platform = self.installed_platform(&entry).ok_or_else(|| {
            Error::msg("the manifest of the installed version is missing; reinstall the game")
        })?;
        progress(Progress::local(Phase::Verify, 0, platform.total_size));
        let bad_files = {
            let (platform, dir) = (platform.clone(), installed.dir.clone());
            let progress = progress.clone();
            tokio::task::spawn_blocking(move || {
                let total = platform.total_size;
                let checked = std::sync::atomic::AtomicU64::new(0);
                install::verify_tree_with(&platform, &dir, &|n| {
                    let done = checked.fetch_add(n, std::sync::atomic::Ordering::Relaxed) + n;
                    progress(Progress::local(Phase::Verify, done.min(total), total));
                })
            })
            .await
            .map_err(|e| Error::msg(format!("verify task failed: {e}")))??
        };
        let mut report = VerifyReport {
            game_id: entry.game_id.clone(),
            checked_files: platform.files.len(),
            bad_files,
            repaired: false,
            downloaded_bytes: 0,
        };
        if report.bad_files.is_empty() || !repair {
            return Ok(report);
        }

        self.stop_seeding(&entry.game_id).await;
        let plan = install::plan_repair(&platform, &report.bad_files);
        // Good chunks inside damaged files do not need to be downloaded again.
        {
            let (platform, dir, store, chunks) = (
                platform.clone(),
                installed.dir.clone(),
                self.store.clone(),
                plan.chunks.clone(),
            );
            let progress = progress.clone();
            tokio::task::spawn_blocking(move || {
                install::harvest_with(&platform, &dir, &chunks, &store, |done, total| {
                    progress(Progress::local(Phase::Harvest, done, total));
                })
            })
            .await
            .map_err(|e| Error::msg(format!("harvest task failed: {e}")))??;
        }
        let missing = install::missing_chunks(&plan, &self.store);
        if !missing.is_empty() {
            let (resolved, entry) = self.resolve(&entry).await?;
            let bases = self.chunk_bases(&entry, &resolved.client).await;
            let stats = fetch_chunks(
                &self.http,
                &bases,
                &self.store,
                &missing,
                &self.download_options(),
                progress.clone(),
            )
            .await?;
            report.downloaded_bytes = stats.bytes;
        }
        {
            let (platform, dir, store, plan) = (
                platform.clone(),
                installed.dir.clone(),
                self.store.clone(),
                plan.clone(),
            );
            let progress = progress.clone();
            tokio::task::spawn_blocking(move || {
                let total = plan.write_bytes(&platform);
                let mut done = 0;
                install::apply_install(&platform, &dir, &store, &plan, |n| {
                    done += n;
                    progress(Progress::local(Phase::Assemble, done, total));
                })
            })
            .await
            .map_err(|e| Error::msg(format!("repair task failed: {e}")))??;
        }
        if !self.settings().keep_chunks {
            for chunk in &plan.chunks {
                let _ = self.store.remove(&chunk.id);
            }
        }
        report.repaired = true;
        if self.settings().seed {
            let _ = self.seed_game_locked(&entry.game_id).await;
        }
        Ok(report)
    }

    // ---- launch ------------------------------------------------------------

    /// A Wine-compatible runner: the configured one, else `wine` on `PATH`.
    pub fn find_wine(&self) -> Option<PathBuf> {
        if let Some(path) = self.settings().wine {
            return path.is_file().then_some(path);
        }
        let path_var = std::env::var_os("PATH")?;
        std::env::split_paths(&path_var)
            .map(|dir| dir.join("wine"))
            .find(|candidate| candidate.is_file())
    }

    /// Build the command that starts the game (not yet spawned).
    pub fn launch_command(
        &self,
        selector: &str,
        extra_args: &[String],
    ) -> Result<std::process::Command> {
        let entry = self.find(selector)?;
        let installed = entry
            .installed
            .as_ref()
            .ok_or_else(|| Error::msg(format!("{} is not installed", entry.slug)))?;
        let platform = self.installed_platform(&entry).ok_or_else(|| {
            Error::msg("the manifest of the installed version is missing; reinstall the game")
        })?;
        let exe = gamedist_core::paths::join_no_symlinks(&installed.dir, &platform.launch.exe)?;
        if !exe.is_file() {
            return Err(Error::msg(format!(
                "{} is missing; verify the game files",
                exe.display()
            )));
        }
        let cwd = if platform.launch.cwd == "." {
            installed.dir.clone()
        } else {
            gamedist_core::paths::join_no_symlinks(&installed.dir, &platform.launch.cwd)?
        };
        let needs_runner =
            installed.platform.starts_with("windows-") && !cfg!(target_os = "windows");
        let mut command = if needs_runner {
            let wine = self.find_wine().ok_or_else(|| {
                Error::msg("this is a Windows build; install Wine or set a runner in Settings")
            })?;
            let mut command = std::process::Command::new(wine);
            command.arg(&exe);
            command
        } else {
            std::process::Command::new(&exe)
        };
        command
            .args(&platform.launch.args)
            .args(extra_args)
            .current_dir(cwd);
        Ok(command)
    }

    /// Start the game and return the child process.
    pub fn launch(&self, selector: &str, extra_args: &[String]) -> Result<std::process::Child> {
        self.launch_command(selector, extra_args)?
            .spawn()
            .map_err(|e| Error::msg(format!("starting the game failed: {e}")))
    }

    // ---- uninstall ---------------------------------------------------------

    /// Remove the game's files (files the game created itself are left) and,
    /// with `forget`, its library entry and key pin.
    pub async fn uninstall(&self, selector: &str, forget: bool) -> Result<()> {
        let game_id = self.find(selector)?.game_id;
        let lock = self.op_lock(&game_id);
        let _guard = lock.lock().await;
        let mut entry = self.find(&game_id)?;
        self.stop_seeding(&entry.game_id).await;
        if let Some(installed) = entry.installed.take() {
            if let Some(platform) = self
                .load_manifest(&entry.game_id, &installed.version)
                .ok()
                .and_then(|mut m| m.platforms.remove(&installed.platform))
            {
                let dir = installed.dir.clone();
                tokio::task::spawn_blocking(move || install::remove_tree(&platform, &dir))
                    .await
                    .map_err(|e| Error::msg(format!("uninstall task failed: {e}")))??;
            }
            let _ = std::fs::remove_file(installed.dir.join(library::INSTALLED_MARKER));
            let _ = std::fs::remove_dir(&installed.dir); // only if now empty
            let _ = std::fs::remove_dir_all(self.data_dir.join("manifests").join(&entry.game_id));
        }
        {
            let mut lib = self.lib();
            if forget {
                lib.games.remove(&entry.game_id);
            } else {
                lib.games.insert(entry.game_id.clone(), entry);
            }
        }
        self.save()
    }

    // ---- BitTorrent -------------------------------------------------------

    #[cfg(feature = "torrent")]
    async fn engine(&self) -> Result<&torrent::TorrentEngine> {
        self.engine
            .get_or_try_init(|| async {
                let settings = self.settings();
                torrent::TorrentEngine::start(torrent::engine_options(
                    self.data_dir.join("torrent"),
                    &settings,
                ))
                .await
            })
            .await
    }

    /// The server's torrent for one platform of a release, validated against
    /// the signed manifest. The torrent itself is unsigned and untrusted.
    #[cfg(feature = "torrent")]
    async fn fetch_metainfo(
        &self,
        client: &ApiClient,
        manifest: &ReleaseManifest,
        platform_name: &str,
    ) -> Result<(Vec<u8>, torrent::TorrentCheck)> {
        let list = client.releases(&manifest.slug).await?;
        let reference = list
            .releases
            .iter()
            .find(|r| r.version == manifest.version)
            .and_then(|r| r.torrents.get(platform_name))
            .ok_or_else(|| Error::msg("the server has no torrent for this release (yet)"))?;
        // Only fetch torrents from the host we are already talking to.
        if !reference.url.starts_with(&format!("{}/", client.base())) {
            return Err(Error::msg("torrent URL points at a different host"));
        }
        let resp = self
            .http
            .get(&reference.url)
            .send()
            .await
            .map_err(|e| Error::msg(format!("fetching torrent: {}", e.without_url())))?;
        let mut resp = gamedist_core::client::check(resp).await?;
        let mut bytes = Vec::new();
        while let Some(part) = resp
            .chunk()
            .await
            .map_err(|e| Error::msg(format!("fetching torrent: {}", e.without_url())))?
        {
            if bytes.len() + part.len() > torrent::MAX_METAINFO_SIZE {
                return Err(Error::msg("torrent file is too large"));
            }
            bytes.extend_from_slice(&part);
        }
        let platform = manifest
            .platforms
            .get(platform_name)
            .ok_or_else(|| Error::msg("platform missing from manifest"))?;
        let check = torrent::check_metainfo(&bytes, platform)?;
        Ok((bytes, check))
    }

    /// Download the files of `plan` through BitTorrent into a scratch
    /// directory and lift every chunk that verifies into the store.
    /// Returns the verified bytes obtained.
    #[cfg(feature = "torrent")]
    async fn torrent_fetch(
        &self,
        client: &ApiClient,
        manifest: &ReleaseManifest,
        platform_name: &str,
        plan: &install::InstallPlan,
        missing: &[gamedist_core::manifest::ChunkRef],
        progress: &ProgressFn,
    ) -> Result<u64> {
        let (metainfo, check) = self.fetch_metainfo(client, manifest, platform_name).await?;
        let platform = manifest.platforms[platform_name].clone();
        let engine = self.engine().await?;
        let scratch = self
            .data_dir
            .join("torrent")
            .join(format!("dl-{}", check.info_hash_hex()));
        let _ = std::fs::remove_dir_all(&scratch);
        let stall = std::time::Duration::from_secs(self.settings().torrent_stall_secs.max(1));
        let report = progress.clone();
        let outcome = engine
            .download(
                metainfo,
                &scratch,
                plan.write.clone(),
                stall,
                move |done, total| {
                    report(Progress::download(
                        gamedist_core::progress::Source::Peers,
                        done,
                        total,
                    ))
                },
            )
            .await;
        // Whatever arrived is untrusted until each chunk has been hashed.
        let harvested = {
            let (scratch, store, missing) = (scratch.clone(), self.store.clone(), missing.to_vec());
            tokio::task::spawn_blocking(move || {
                let result = install::harvest(&platform, &scratch, &missing, &store);
                let _ = std::fs::remove_dir_all(&scratch);
                result
            })
            .await
            .map_err(|e| Error::msg(format!("harvest task failed: {e}")))??
        };
        outcome?;
        Ok(harvested)
    }

    #[cfg(not(feature = "torrent"))]
    async fn torrent_fetch(
        &self,
        _client: &ApiClient,
        _manifest: &ReleaseManifest,
        _platform_name: &str,
        _plan: &install::InstallPlan,
        _missing: &[gamedist_core::manifest::ChunkRef],
        _progress: &ProgressFn,
    ) -> Result<u64> {
        Err(Error::msg("this build has no BitTorrent support"))
    }

    /// Seed an installed game. The install is first verified against the
    /// signed manifest, and the torrent is only used if its piece hashes
    /// equal the ones computed from the verified files.
    pub async fn seed_game(&self, selector: &str) -> Result<()> {
        let game_id = self.find(selector)?.game_id;
        let lock = self.op_lock(&game_id);
        let _guard = lock.lock().await;
        self.seed_game_locked(&game_id).await
    }

    #[cfg(feature = "torrent")]
    async fn seed_game_locked(&self, selector: &str) -> Result<()> {
        let entry = self.find(selector)?;
        let installed = entry
            .installed
            .clone()
            .ok_or_else(|| Error::msg(format!("{} is not installed", entry.slug)))?;
        let manifest = self.load_manifest(&entry.game_id, &installed.version)?;
        let platform = manifest
            .platforms
            .get(&installed.platform)
            .cloned()
            .ok_or_else(|| Error::msg("installed platform missing from manifest"))?;
        let (resolved, entry) = self.resolve(&entry).await?;
        let (metainfo, check) = self
            .fetch_metainfo(&resolved.client, &manifest, &installed.platform)
            .await?;
        let matches = {
            let (platform, dir, check) = (platform.clone(), installed.dir.clone(), check.clone());
            tokio::task::spawn_blocking(move || -> Result<bool> {
                if !install::verify_tree(&platform, &dir)?.is_empty() {
                    return Ok(false);
                }
                Ok(torrent::tree_pieces(&platform, &dir, check.piece_length)? == check.pieces)
            })
            .await
            .map_err(|e| Error::msg(format!("verify task failed: {e}")))??
        };
        if !matches {
            // Never keep offering data that no longer matches the release.
            self.stop_seeding(&entry.game_id).await;
            return Err(Error::msg(format!(
                "not seeding {}: the installed files or the server's torrent do not match the signed release",
                entry.slug
            )));
        }
        self.engine()
            .await?
            .seed(
                &entry.game_id,
                metainfo,
                &installed.dir,
                &check.info_hash_hex(),
            )
            .await
    }

    #[cfg(not(feature = "torrent"))]
    async fn seed_game_locked(&self, _selector: &str) -> Result<()> {
        Err(Error::msg("this build has no BitTorrent support"))
    }

    /// Seed every installed game (when seeding is enabled in the settings).
    /// Returns the games that could not be seeded, with the reason.
    pub async fn start_seeding(&self) -> Vec<(String, String)> {
        let mut failures = Vec::new();
        if !self.settings().seed {
            return failures;
        }
        for game in self.games() {
            if game.installed.is_some() {
                if let Err(e) = self.seed_game(&game.game_id).await {
                    failures.push((game.slug.clone(), e.to_string()));
                }
            }
        }
        failures
    }

    /// Stop seeding a game; a no-op if it is not being seeded.
    pub async fn stop_seeding(&self, game_id: &str) {
        #[cfg(feature = "torrent")]
        if let Some(engine) = self.engine.get() {
            engine.stop(game_id).await;
        }
        #[cfg(not(feature = "torrent"))]
        let _ = game_id;
    }

    /// Upload statistics of the games currently being seeded.
    pub fn seeding(&self) -> Vec<torrent::SeedStatus> {
        #[cfg(feature = "torrent")]
        if let Some(engine) = self.engine.get() {
            return engine.status();
        }
        Vec::new()
    }

    /// The BitTorrent listen port, once the engine is running.
    pub fn torrent_port(&self) -> Option<u16> {
        #[cfg(feature = "torrent")]
        if let Some(engine) = self.engine.get() {
            return engine.listen_port();
        }
        None
    }

    /// Stop the BitTorrent session (call before the process exits).
    pub async fn shutdown(&self) {
        #[cfg(feature = "torrent")]
        if let Some(engine) = self.engine.get() {
            engine.shutdown().await;
        }
    }

    // ---- catalogs ----------------------------------------------------------

    /// Add a server whose catalog appears in Discover. Returns its name.
    pub async fn add_catalog(&self, server: &str) -> Result<String> {
        let client = ApiClient::connect_with(self.http.clone(), server).await?;
        if client.info().catalog.is_none() {
            return Err(Error::msg(format!(
                "{} does not publish a catalog",
                client.info().name
            )));
        }
        let base = client.base().to_string();
        {
            let mut lib = self.lib();
            if !lib.catalogs.contains(&base) {
                lib.catalogs.push(base);
            }
        }
        self.save()?;
        Ok(client.info().name.clone())
    }

    pub fn remove_catalog(&self, server: &str) -> Result<()> {
        let server = server.trim_end_matches('/');
        self.lib().catalogs.retain(|c| c != server);
        self.save()
    }

    pub fn catalogs(&self) -> Vec<String> {
        self.lib().catalogs.clone()
    }

    /// Search every configured catalog. Unreachable servers are skipped and
    /// reported in the second element.
    pub async fn discover(&self, query: &str) -> (Vec<DiscoverEntry>, Vec<String>) {
        let mut found = Vec::new();
        let mut errors = Vec::new();
        for server in self.catalogs() {
            let result = async {
                let client = ApiClient::connect_with(self.http.clone(), &server).await?;
                let page = client.catalog(query, 1).await?;
                Ok::<_, Error>((
                    client.info().name.clone(),
                    client.info().chunks.clone(),
                    page.games,
                ))
            }
            .await;
            match result {
                Ok((name, asset_base, games)) => {
                    let lib = self.lib();
                    found.extend(games.into_iter().map(|game| DiscoverEntry {
                        in_library: lib.games.contains_key(&game.game_id),
                        asset_base: asset_base.clone(),
                        server: server.clone(),
                        server_name: name.clone(),
                        game,
                    }));
                }
                Err(e) => errors.push(format!("{server}: {e}")),
            }
        }
        (found, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUI runs these on a multi-threaded runtime: every public future
    /// must be `Send` (no lock guard may be held across an await).
    #[test]
    fn public_futures_are_send() {
        fn assert_send<T: Send>(_: T) {}
        let tmp = tempfile::tempdir().unwrap();
        let launcher = Launcher::open(tmp.path()).unwrap();
        let l = &launcher;
        assert_send(l.add_game("https://h/games/xx", false));
        assert_send(l.check_update("x"));
        assert_send(l.install(
            "x",
            InstallOptions::default(),
            gamedist_core::progress::no_progress(),
        ));
        assert_send(l.verify("x", true, gamedist_core::progress::no_progress()));
        assert_send(l.uninstall("x", false));
        assert_send(l.add_catalog("https://h"));
        assert_send(l.discover(""));
        assert_send(l.seed_game("x"));
        assert_send(l.start_seeding());
        assert_send(l.stop_seeding("x"));
        assert_send(l.shutdown());
    }

    #[test]
    fn find_by_id_slug_and_url() {
        let tmp = tempfile::tempdir().unwrap();
        let launcher = Launcher::open(tmp.path()).unwrap();
        let key = gamedist_core::SecretKey::generate().public();
        let entry: LibraryEntry = serde_json::from_value(serde_json::json!({
            "game_id": gamedist_core::game_id(&key, "cool-game"),
            "slug": "cool-game",
            "title": "Cool",
            "sources": ["https://a.example"],
            "pinned_key": key.to_string(),
            "channel": "stable"
        }))
        .unwrap();
        launcher.put_entry(&entry).unwrap();
        assert_eq!(launcher.find(&entry.game_id).unwrap(), entry);
        assert_eq!(launcher.find("cool-game").unwrap(), entry);
        assert_eq!(
            launcher
                .find("gamedist://a.example/games/cool-game")
                .unwrap(),
            entry
        );
        assert!(launcher.find("https://b.example/games/cool-game").is_err());
        assert!(launcher.find("other").is_err());
        // Not installed: launching and verifying say so.
        assert!(launcher
            .launch_command("cool-game", &[])
            .unwrap_err()
            .to_string()
            .contains("not installed"));
        // Reopening reads the same state.
        drop(launcher);
        assert_eq!(Launcher::open(tmp.path()).unwrap().games(), vec![entry]);
    }
}
