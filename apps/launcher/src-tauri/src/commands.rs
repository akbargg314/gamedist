//! Tauri commands, events and app wiring.
//!
//! Every command returns `Result<_, CmdError>`; the error serialises as
//! `{ kind: "key_changed" | "error", message, pinned?, presented?, slug?, source_url? }`
//! which the frontend (`src/lib/api.ts`) turns into an `ApiError`.
//!
//! Events:
//! * `gamedist://progress` `{ game_id, op: "install"|"verify", phase, done, total }` (≤ ~10/s)
//! * `gamedist://exited`   `{ game_id, code }` when a launched game process ends
//! * `gamedist://open-url` `[url, …]` when a deep link arrives while running
//! * `gamedist://seed-failures` `[{ slug, reason }, …]` when startup seeding
//!   finishes after the UI called `take_seed_failures` (otherwise the list is
//!   returned by that call)

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use gamedist_core::progress::{Phase, Progress, ProgressFn, Source};
use gamedist_launcher::{torrent::SeedStatus, InstallOptions, Launcher, Settings, Transport};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;

pub const EVENT_PROGRESS: &str = "gamedist://progress";
pub const EVENT_EXITED: &str = "gamedist://exited";
pub const EVENT_OPEN_URL: &str = "gamedist://open-url";
pub const EVENT_SEED_FAILURES: &str = "gamedist://seed-failures";

/// Deep-link schemes we act on (must match `plugins.deep-link.desktop.schemes`).
const SCHEMES: &[&str] = &["gamedist", "gamedist+http"];

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Upper bound on how long app exit waits for the torrent session to stop.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// State

pub struct AppState {
    launcher: Arc<Launcher>,
    running: Arc<Mutex<HashSet<String>>>,
    /// Deep links received before the frontend asked for them.
    pending_links: Mutex<Vec<String>>,
    /// Set once the frontend has called `take_pending_links`.
    frontend_ready: AtomicBool,
    /// Results of the startup `start_seeding` run (see `take_seed_failures`).
    startup_seed: Mutex<StartupSeed>,
}

#[derive(Default)]
struct StartupSeed {
    /// The UI has called `take_seed_failures`; later results go out as an event.
    ui_listening: bool,
    /// Failures that finished before the UI was listening.
    failures: Vec<SeedFailure>,
}

impl AppState {
    fn launcher(&self) -> Arc<Launcher> {
        Arc::clone(&self.launcher)
    }
}

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CmdError {
    KeyChanged {
        message: String,
        slug: String,
        source_url: String,
        pinned: String,
        presented: String,
    },
    Error {
        message: String,
    },
}

impl CmdError {
    fn msg(message: impl Into<String>) -> Self {
        CmdError::Error { message: message.into() }
    }
}

impl From<gamedist_launcher::Error> for CmdError {
    fn from(e: gamedist_launcher::Error) -> Self {
        let message = e.to_string();
        match e {
            gamedist_launcher::Error::KeyChanged {
                slug,
                source_url,
                pinned,
                presented,
                ..
            } => CmdError::KeyChanged {
                message,
                slug: slug.to_string(),
                source_url: source_url.to_string(),
                pinned: pinned.to_string(),
                presented: presented.to_string(),
            },
            #[allow(unreachable_patterns)]
            _ => CmdError::Error { message },
        }
    }
}

impl From<serde_json::Error> for CmdError {
    fn from(e: serde_json::Error) -> Self {
        CmdError::msg(format!("serialising response: {e}"))
    }
}

impl From<tauri::Error> for CmdError {
    fn from(e: tauri::Error) -> Self {
        CmdError::msg(e.to_string())
    }
}

type CmdResult<T> = Result<T, CmdError>;

/// Serialise engine types without naming every one of them here.
fn json<T: Serialize>(v: T) -> CmdResult<Value> {
    Ok(serde_json::to_value(v)?)
}

/// Run blocking engine work (filesystem-heavy) off the async runtime threads.
async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CmdError::msg(format!("background task failed: {e}")))?
}

// ---------------------------------------------------------------------------
// Progress

#[derive(Clone, Serialize)]
struct ProgressPayload {
    game_id: String,
    op: &'static str,
    phase: Phase,
    /// "http" | "peers" for the download phase, otherwise null.
    source: Option<Source>,
    done: u64,
    total: u64,
}

/// A ProgressFn that forwards to the UI, throttled to ~10 events/s. Phase
/// changes and completion are always delivered.
fn progress_emitter(app: AppHandle, game_id: String, op: &'static str) -> ProgressFn {
    let last: Mutex<Option<LastProgress>> = Mutex::new(None);
    Arc::new(move |p: Progress| {
        let now = Instant::now();
        // The lock is held while emitting so events leave in the order they
        // were accepted; reports come from several worker threads.
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        let same_step = last.is_some_and(|l| l.phase == p.phase && l.source == p.source);
        if same_step {
            let l = last.expect("checked above");
            // Counters of the disk phases only grow: a lower value is a
            // report that lost a race with a later one. (Download counters
            // do go down, when a failed attempt is un-counted.)
            if p.phase != Phase::Download && p.done < l.done {
                return;
            }
            let finished = p.phase == Phase::Done || (p.total > 0 && p.done >= p.total);
            if !finished && now.duration_since(l.at) < PROGRESS_INTERVAL {
                return;
            }
        }
        *last = Some(LastProgress {
            at: now,
            phase: p.phase,
            source: p.source,
            done: p.done,
        });
        let _ = app.emit(
            EVENT_PROGRESS,
            ProgressPayload {
                game_id: game_id.clone(),
                op,
                phase: p.phase,
                source: p.source,
                done: p.done,
                total: p.total,
            },
        );
    })
}

/// The last progress event sent for one operation.
#[derive(Clone, Copy)]
struct LastProgress {
    at: Instant,
    phase: Phase,
    source: Option<Source>,
    done: u64,
}

// ---------------------------------------------------------------------------
// Commands: settings & library

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> CmdResult<Value> {
    json(state.launcher.settings())
}

/// `Some(value)` for a present field, including `null` (`Some(None)`), so a
/// patch can tell "set to unlimited" apart from "not edited".
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// The settings fields the UI changed. Absent fields keep their current value,
/// so settings the UI does not show (e.g. `torrent_stall_secs`) survive a save.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SettingsPatch {
    install_root: Option<PathBuf>,
    #[serde(deserialize_with = "present")]
    bandwidth_limit: Option<Option<u64>>,
    concurrency: Option<usize>,
    keep_chunks: Option<bool>,
    transport: Option<Transport>,
    seed: Option<bool>,
    #[serde(deserialize_with = "present")]
    upload_limit: Option<Option<u64>>,
    #[serde(deserialize_with = "present")]
    wine: Option<Option<PathBuf>>,
    listen_port: Option<u16>,
    torrent_stall_secs: Option<u64>,
}

impl SettingsPatch {
    fn apply(self, s: &mut Settings) {
        if let Some(v) = self.install_root {
            s.install_root = v;
        }
        if let Some(v) = self.bandwidth_limit {
            s.bandwidth_limit = v;
        }
        if let Some(v) = self.concurrency {
            s.concurrency = v;
        }
        if let Some(v) = self.keep_chunks {
            s.keep_chunks = v;
        }
        if let Some(v) = self.transport {
            s.transport = v;
        }
        if let Some(v) = self.seed {
            s.seed = v;
        }
        if let Some(v) = self.upload_limit {
            s.upload_limit = v;
        }
        if let Some(v) = self.wine {
            s.wine = v;
        }
        if let Some(v) = self.listen_port {
            s.listen_port = v;
        }
        if let Some(v) = self.torrent_stall_secs {
            s.torrent_stall_secs = v;
        }
    }
}

/// Apply a partial update to the current settings and return the result.
/// Turning seeding off stops every running seed. Turning it on does not start
/// anything here; the UI then calls `start_seeding` and shows its failures.
#[tauri::command]
pub async fn update_settings(state: State<'_, AppState>, patch: SettingsPatch) -> CmdResult<Settings> {
    let l = state.launcher();
    let was_seeding = l.settings().seed;
    let saved = {
        let l = Arc::clone(&l);
        blocking(move || Ok(l.update_settings(move |s| patch.apply(s))?)).await?
    };
    if was_seeding && !saved.seed {
        for status in l.seeding() {
            l.stop_seeding(&status.game_id).await;
        }
    }
    Ok(saved)
}

#[tauri::command]
pub async fn list_games(state: State<'_, AppState>) -> CmdResult<Value> {
    json(state.launcher.games())
}

#[tauri::command]
pub async fn find_game(state: State<'_, AppState>, selector: String) -> CmdResult<Value> {
    json(state.launcher.find(&selector)?)
}

#[tauri::command]
pub async fn add_game(state: State<'_, AppState>, url: String, accept_key_change: bool) -> CmdResult<Value> {
    let l = state.launcher();
    json(l.add_game(url.trim(), accept_key_change).await?)
}

#[tauri::command]
pub async fn check_update(state: State<'_, AppState>, selector: String) -> CmdResult<Value> {
    let l = state.launcher();
    json(l.check_update(&selector).await?)
}

/// Install options as sent by the UI (the engine type is not Deserialize).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct InstallOptionsDto {
    dir: Option<PathBuf>,
    platform: Option<String>,
    channel: Option<String>,
    version: Option<String>,
    allow_downgrade: bool,
}

impl From<InstallOptionsDto> for InstallOptions {
    #[allow(clippy::needless_update)]
    fn from(o: InstallOptionsDto) -> Self {
        let blank = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
        InstallOptions {
            dir: o.dir.filter(|d| !d.as_os_str().is_empty()),
            platform: blank(o.platform),
            channel: blank(o.channel),
            version: blank(o.version),
            allow_downgrade: o.allow_downgrade,
            ..Default::default()
        }
    }
}

/// Install or update. Progress goes out as `gamedist://progress` with op "install".
#[tauri::command]
pub async fn install_game(
    app: AppHandle,
    state: State<'_, AppState>,
    selector: String,
    opts: Option<InstallOptionsDto>,
) -> CmdResult<Value> {
    let l = state.launcher();
    let game_id = l.find(&selector)?.game_id;
    let progress = progress_emitter(app, game_id, "install");
    let report = l.install(&selector, opts.unwrap_or_default().into(), progress).await?;
    json(report)
}

#[tauri::command]
pub async fn verify_game(
    app: AppHandle,
    state: State<'_, AppState>,
    selector: String,
    repair: bool,
) -> CmdResult<Value> {
    let l = state.launcher();
    let game_id = l.find(&selector)?.game_id;
    let progress = progress_emitter(app, game_id, "verify");
    json(l.verify(&selector, repair, progress).await?)
}

#[derive(Clone, Serialize)]
struct ExitedPayload {
    game_id: String,
    code: Option<i32>,
}

/// Spawn the game; `gamedist://exited` fires when the process ends.
#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    state: State<'_, AppState>,
    selector: String,
    extra_args: Option<Vec<String>>,
) -> CmdResult<()> {
    let l = state.launcher();
    let game_id = l.find(&selector)?.game_id;
    let running = Arc::clone(&state.running);
    if running.lock().unwrap_or_else(|e| e.into_inner()).contains(&game_id) {
        return Err(CmdError::msg("this game is already running"));
    }
    let mut child = l.launch(&selector, &extra_args.unwrap_or_default())?;
    running.lock().unwrap_or_else(|e| e.into_inner()).insert(game_id.clone());
    std::thread::Builder::new()
        .name(format!("wait-{game_id}"))
        .spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code());
            running.lock().unwrap_or_else(|e| e.into_inner()).remove(&game_id);
            let _ = app.emit(EVENT_EXITED, ExitedPayload { game_id, code });
        })
        .map_err(|e| CmdError::msg(format!("could not watch game process: {e}")))?;
    Ok(())
}

#[tauri::command]
pub async fn running_games(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    Ok(state
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect())
}

#[tauri::command]
pub async fn uninstall_game(state: State<'_, AppState>, selector: String, forget: bool) -> CmdResult<()> {
    let l = state.launcher();
    let game_id = l.find(&selector)?.game_id;
    if state.running.lock().unwrap_or_else(|e| e.into_inner()).contains(&game_id) {
        return Err(CmdError::msg("quit the game before uninstalling it"));
    }
    // The engine stops seeding the game before touching its files.
    l.uninstall(&selector, forget).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands: seeding

#[derive(Debug, Clone, Serialize)]
pub struct SeedFailure {
    slug: String,
    reason: String,
}

fn seed_failures(raw: Vec<(String, String)>) -> Vec<SeedFailure> {
    raw.into_iter()
        .map(|(slug, reason)| SeedFailure { slug, reason })
        .collect()
}

/// Seed every installed game (a no-op unless `settings.seed`). Returns the
/// games that could not be seeded. Slow: each install is re-verified first.
#[tauri::command]
pub async fn start_seeding(state: State<'_, AppState>) -> CmdResult<Vec<SeedFailure>> {
    let l = state.launcher();
    Ok(seed_failures(l.start_seeding().await))
}

#[tauri::command]
pub async fn seed_game(state: State<'_, AppState>, selector: String) -> CmdResult<()> {
    let l = state.launcher();
    Ok(l.seed_game(&selector).await?)
}

#[tauri::command]
pub async fn stop_seeding(state: State<'_, AppState>, game_id: String) -> CmdResult<()> {
    let l = state.launcher();
    l.stop_seeding(&game_id).await;
    Ok(())
}

#[derive(Serialize)]
struct SeedingInfo {
    games: Vec<SeedStatus>,
    /// BitTorrent listen port; `null` until the torrent engine has started.
    port: Option<u16>,
}

#[tauri::command]
pub async fn seeding_status(state: State<'_, AppState>) -> CmdResult<Value> {
    json(SeedingInfo {
        games: state.launcher.seeding(),
        port: state.launcher.torrent_port(),
    })
}

/// Called once by the UI after it subscribed to `gamedist://seed-failures`:
/// returns startup seeding failures that happened before; later ones arrive
/// as that event.
#[tauri::command]
pub async fn take_seed_failures(state: State<'_, AppState>) -> CmdResult<Vec<SeedFailure>> {
    let mut startup = state.startup_seed.lock().unwrap_or_else(|e| e.into_inner());
    startup.ui_listening = true;
    Ok(std::mem::take(&mut startup.failures))
}

/// Startup seeding finished: emit (UI listening) or queue (not yet) failures.
fn deliver_seed_failures(app: &AppHandle, failures: Vec<SeedFailure>) {
    if failures.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    // Decide under the lock so a concurrent `take_seed_failures` cannot miss them.
    let mut startup = state.startup_seed.lock().unwrap_or_else(|e| e.into_inner());
    if startup.ui_listening {
        let _ = app.emit(EVENT_SEED_FAILURES, failures);
    } else {
        startup.failures.extend(failures);
    }
}

// ---------------------------------------------------------------------------
// Commands: disk space

#[derive(Debug, Serialize)]
pub struct DiskSpace {
    /// The directory that was measured: `path` itself or, if it does not
    /// exist yet, its nearest existing ancestor.
    path: PathBuf,
    total_bytes: u64,
    /// Space available to this user (excludes blocks reserved for root).
    free_bytes: u64,
    /// Opaque filesystem identity: two paths with the same value are on the
    /// same filesystem (device number on Unix, drive / share on Windows).
    volume: String,
}

/// Free and total space of the filesystem that holds `path` (which need not
/// exist yet, e.g. the install folder of a game that is about to be installed).
#[tauri::command]
pub async fn disk_space(path: PathBuf) -> CmdResult<DiskSpace> {
    blocking(move || {
        let abs = std::path::absolute(&path)
            .map_err(|e| CmdError::msg(format!("{}: {e}", path.display())))?;
        let dir = abs
            .ancestors()
            .find(|p| p.is_dir())
            .ok_or_else(|| CmdError::msg(format!("no existing folder above {}", abs.display())))?
            .to_path_buf();
        let stats =
            fs4::statvfs(&dir).map_err(|e| CmdError::msg(format!("{}: {e}", dir.display())))?;
        Ok(DiskSpace {
            volume: volume_id(&dir),
            total_bytes: stats.total_space(),
            free_bytes: stats.available_space(),
            path: dir,
        })
    })
    .await
}

#[cfg(unix)]
fn volume_id(dir: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(dir)
        .map(|m| m.dev().to_string())
        .unwrap_or_default()
}

/// The path prefix (`C:` or `\\server\share`). Folder mount points are rare
/// enough to ignore here.
#[cfg(not(unix))]
fn volume_id(dir: &Path) -> String {
    dir.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Commands: catalogs

#[tauri::command]
pub async fn list_catalogs(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    Ok(state.launcher.catalogs())
}

#[tauri::command]
pub async fn add_catalog(state: State<'_, AppState>, server: String) -> CmdResult<String> {
    let l = state.launcher();
    Ok(l.add_catalog(server.trim()).await?)
}

#[tauri::command]
pub async fn remove_catalog(state: State<'_, AppState>, server: String) -> CmdResult<()> {
    Ok(state.launcher.remove_catalog(&server)?)
}

#[derive(Serialize)]
struct DiscoverResult<G: Serialize> {
    games: G,
    errors: Vec<String>,
}

#[tauri::command]
pub async fn discover(state: State<'_, AppState>, query: String) -> CmdResult<Value> {
    let l = state.launcher();
    let (games, errors) = l.discover(&query).await;
    json(DiscoverResult { games, errors })
}

// ---------------------------------------------------------------------------
// Deep links

fn is_our_link(url: &str) -> bool {
    url.split_once("://")
        .is_some_and(|(scheme, _)| SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme)))
}

/// Queue (before the UI is ready) or emit (after) incoming deep links.
fn deliver_links(app: &AppHandle, urls: Vec<String>) {
    let urls: Vec<String> = urls.into_iter().filter(|u| is_our_link(u)).collect();
    if urls.is_empty() {
        return;
    }
    focus_main(app);
    let state = app.state::<AppState>();
    if state.frontend_ready.load(Ordering::SeqCst) {
        let _ = app.emit(EVENT_OPEN_URL, urls);
    } else {
        state.pending_links.lock().unwrap_or_else(|e| e.into_inner()).extend(urls);
    }
}

/// Called once by the UI at startup: returns links that arrived before it was listening.
#[tauri::command]
pub async fn take_pending_links(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    let mut pending = state.pending_links.lock().unwrap_or_else(|e| e.into_inner());
    state.frontend_ready.store(true, Ordering::SeqCst);
    Ok(std::mem::take(&mut *pending))
}

fn focus_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ---------------------------------------------------------------------------
// App

pub fn run() -> tauri::Result<()> {
    let app = tauri::Builder::default()
        // Must be registered first. With the "deep-link" feature it forwards a
        // second instance's deep-link argv to `on_open_url` below.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            focus_main(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = Launcher::default_data_dir().map_err(|e| e.to_string())?;
            let launcher = Arc::new(Launcher::open(&data_dir).map_err(|e| e.to_string())?);
            app.manage(AppState {
                launcher: Arc::clone(&launcher),
                running: Arc::new(Mutex::new(HashSet::new())),
                pending_links: Mutex::new(Vec::new()),
                frontend_ready: AtomicBool::new(false),
                startup_seed: Mutex::new(StartupSeed::default()),
            });

            // Resume seeding in the background (a no-op when seeding is off).
            // Re-verifying every install can take a while; the UI is not blocked.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let failures = seed_failures(launcher.start_seeding().await);
                deliver_seed_failures(&handle, failures);
            });

            // Linux and Windows dev builds register the URL schemes at runtime
            // (installers do it for release builds; on Linux this also covers
            // AppImages). macOS only supports bundle-time registration.
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            if let Err(e) = app.deep_link().register_all() {
                eprintln!("gamedist-launcher: could not register URL schemes: {e}");
            }

            // Links the app was started with.
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                deliver_links(app.handle(), urls.iter().map(|u| u.to_string()).collect());
            }
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                deliver_links(&handle, event.urls().iter().map(|u| u.to_string()).collect());
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            update_settings,
            list_games,
            find_game,
            add_game,
            check_update,
            install_game,
            verify_game,
            launch_game,
            running_games,
            uninstall_game,
            start_seeding,
            seed_game,
            stop_seeding,
            seeding_status,
            take_seed_failures,
            disk_space,
            list_catalogs,
            add_catalog,
            remove_catalog,
            discover,
            take_pending_links,
        ])
        .build(tauri::generate_context!())?;

    app.run(|handle, event| {
        // `Exit` fires once the event loop is ending, whichever way the app
        // was asked to quit (last window closed, Cmd+Q, `AppHandle::exit`).
        // This callback runs on the main thread, not on an async runtime
        // worker, so blocking on the shutdown future here is fine.
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<AppState>() {
                let launcher = state.launcher();
                tauri::async_runtime::block_on(async move {
                    if tokio::time::timeout(SHUTDOWN_TIMEOUT, launcher.shutdown())
                        .await
                        .is_err()
                    {
                        eprintln!("gamedist-launcher: torrent session did not stop in time");
                    }
                });
            }
        }
    });
    Ok(())
}
