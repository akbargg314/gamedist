//! End-to-end tests across the real server, the developer CLI, the launcher
//! engine / headless CLI and the sample game.
//!
//! These are the acceptance tests of PLAN.md phases 2, 3 and 5.

use gamedist_cli::{
    commands::{self, Ctx, InitOptions, LoginOptions, PushOptions},
    config::Credentials,
};
use gamedist_core::{
    chunker::ChunkParams,
    manifest::Registration,
    progress::{no_progress, Phase, Progress, ProgressFn, Source},
};
use gamedist_launcher::{headless, Error as LauncherError, InstallOptions, Launcher};
use gamedist_server::{state::St, Config, Server};
use std::path::{Path, PathBuf};

const PASSWORD: &str = "correct horse battery";

type Seen = std::sync::Arc<std::sync::Mutex<Vec<Progress>>>;

/// A progress callback that records every report.
fn recorder() -> (ProgressFn, Seen) {
    let seen = Seen::default();
    let sink = seen.clone();
    (
        std::sync::Arc::new(move |p| sink.lock().unwrap().push(p)),
        seen,
    )
}
const EXE: &str = env!("CARGO_BIN_EXE_sample-game-fixture");

struct TestServer {
    url: String,
    state: St,
    _tmp: tempfile::TempDir,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl TestServer {
    async fn start() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let server = Server::bind(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: tmp.path().to_path_buf(),
            registration: Registration::Open,
            auth_rate_per_minute: 10_000,
            ..Config::default()
        })
        .await
        .unwrap();
        let url = server.url().to_string();
        let state = server.state.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            server
                .run(async {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        TestServer {
            url,
            state,
            _tmp: tmp,
            stop: Some(tx),
            task: Some(task),
        }
    }

    /// Take the server offline.
    async fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }
    }
}

/// A developer workstation: project dir, config dir, build dir.
struct Dev {
    ctx: Ctx,
    build: PathBuf,
    _tmp: tempfile::TempDir,
}

fn exe_name() -> String {
    format!("sample-game{}", std::env::consts::EXE_SUFFIX)
}

fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15) | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

const PAK_SIZE: usize = 300_000;

impl Dev {
    async fn new(server: &TestServer, slug: &str, email: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let ctx = Ctx {
            project_dir: tmp.path().join("project"),
            credentials: Credentials::file_only(&config_dir),
            config_dir,
            chunk_params: ChunkParams::small(),
            verbose: false,
        };
        commands::init(
            &ctx,
            InitOptions {
                slug: slug.into(),
                title: Some("Sample Game".into()),
                key: None,
                server: Some(server.url.clone()),
            },
        )
        .unwrap();
        login(&ctx, &server.url, email).await;
        commands::game_create(&ctx, None, "A tiny test game", "")
            .await
            .unwrap();

        let build = tmp.path().join("build");
        std::fs::create_dir_all(build.join("data")).unwrap();
        std::fs::copy(EXE, build.join(exe_name())).unwrap();
        std::fs::write(build.join("data/words.txt"), "apple\nbanana\ncherry\n").unwrap();
        std::fs::write(build.join("data/big.pak"), pseudo_random(PAK_SIZE, 7)).unwrap();
        Dev {
            ctx,
            build,
            _tmp: tmp,
        }
    }

    /// Stamp the build with `version` and publish it to `stable`.
    async fn release(&self, version: &str) -> Vec<commands::PushReport> {
        std::fs::write(self.build.join("data/version.txt"), format!("{version}\n")).unwrap();
        commands::push(
            &self.ctx,
            PushOptions {
                builds: vec![(gamedist_core::ids::host_platform(), self.build.clone())],
                version: version.into(),
                channel: Some("stable".into()),
                changelog: Some(format!("What changed in {version}")),
                server: None,
                exe: Some(exe_name()),
                concurrency: 4,
            },
        )
        .await
        .unwrap()
    }
}

async fn login(ctx: &Ctx, server: &str, email: &str) {
    commands::login(
        ctx,
        LoginOptions {
            server: server.into(),
            email: email.into(),
            password: PASSWORD.into(),
            register: true,
            invite: None,
        },
    )
    .await
    .unwrap();
}

fn assert_same_tree(build: &Path, install: &Path) {
    for rel in ["data/version.txt", "data/words.txt", "data/big.pak"] {
        assert_eq!(
            std::fs::read(build.join(rel)).unwrap(),
            std::fs::read(install.join(rel)).unwrap(),
            "{rel} differs"
        );
    }
    assert_eq!(
        std::fs::read(build.join(exe_name())).unwrap(),
        std::fs::read(install.join(exe_name())).unwrap()
    );
}

fn headless_args<'a>(data_dir: &'a Path, rest: &[&'a str]) -> Vec<String> {
    let mut args = vec![
        "gamedist-launcher".to_string(),
        "--headless".into(),
        "--quiet".into(),
        "--data-dir".into(),
        data_dir.display().to_string(),
    ];
    args.extend(rest.iter().map(|s| s.to_string()));
    args
}

/// Phase 2 acceptance: push v1 → headless install → push v2 with one changed
/// file → update downloads only the changed chunks → launch `--selftest`
/// exits 0.
#[tokio::test]
async fn install_delta_update_and_launch() {
    let server = TestServer::start().await;
    let dev = Dev::new(&server, "sample-game", "dev@example.com").await;
    dev.release("1.0.0").await;
    let url = format!("{}/games/sample-game", server.url);

    let player = tempfile::tempdir().unwrap();
    let data_dir = player.path().join("data");
    let install_dir = player.path().join("games/sample");

    // Headless install straight from the URL.
    let code = headless::run_from(headless_args(
        &data_dir,
        &["install", &url, "--dir", install_dir.to_str().unwrap()],
    ))
    .await
    .unwrap();
    assert_eq!(code, 0);
    assert_same_tree(&dev.build, &install_dir);

    let launcher = Launcher::open(&data_dir).unwrap();
    let entry = launcher.find("sample-game").unwrap();
    assert_eq!(
        entry.pinned_key,
        commands::key_public(&dev.ctx, "sample-game").unwrap()
    );
    assert_eq!(entry.installed.as_ref().unwrap().version, "1.0.0");
    assert_eq!(entry.title, "Sample Game");
    // installed.json marker carries the pin.
    let marker: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(install_dir.join("installed.json")).unwrap())
            .unwrap();
    assert_eq!(marker["version"], "1.0.0");
    assert_eq!(marker["developer_pubkey"], entry.pinned_key.to_string());
    assert_eq!(marker["game_id"], entry.game_id);
    // Chunks are not kept by default.
    assert!(launcher.store().list().unwrap().is_empty());

    // Launch through the headless CLI: the game's exit code is returned.
    let code = headless::run_from(headless_args(
        &data_dir,
        &["launch", "sample-game", "--", "--selftest"],
    ))
    .await
    .unwrap();
    assert_eq!(
        code, 0,
        "sample game selftest must pass on the installed tree"
    );
    let output = launcher
        .launch_command("sample-game", &["--selftest".into()])
        .unwrap()
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "sample-game 1.0.0 selftest ok"
    );

    // Up to date: nothing to do.
    let info = launcher.check_update("sample-game").await.unwrap();
    assert!(!info.update_available);
    assert_eq!(info.download_bytes, 0);
    let noop = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert!(noop.up_to_date);
    assert_eq!((noop.downloaded_bytes, noop.files_written), (0, 0));

    // v2 changes exactly one small file.
    dev.release("2.0.0").await;
    let info = launcher.check_update(&url).await.unwrap();
    assert!(info.update_available);
    assert_eq!(info.installed.as_deref(), Some("1.0.0"));
    assert_eq!(info.latest, "2.0.0");
    assert_eq!(info.download_bytes, 6, "only data/version.txt changed");
    assert_eq!(info.changelog_md, "What changed in 2.0.0");

    let update = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_eq!(update.previous_version.as_deref(), Some("1.0.0"));
    assert_eq!(update.version, "2.0.0");
    assert_eq!(
        update.downloaded_chunks, 1,
        "delta update fetches only the changed chunk"
    );
    assert_eq!(update.downloaded_bytes, 6);
    assert_eq!(update.files_written, 1);
    assert_same_tree(&dev.build, &install_dir);

    let code = headless::run_from(headless_args(
        &data_dir,
        &["launch", "sample-game", "--", "--selftest"],
    ))
    .await
    .unwrap();
    assert_eq!(code, 0);
    let output = launcher
        .launch_command("sample-game", &["--selftest".into()])
        .unwrap()
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "sample-game 2.0.0 selftest ok"
    );

    // v3 inserts bytes into the middle of the big file and drops a file:
    // content-defined chunking keeps most chunks, which are harvested from
    // the installed copy instead of downloaded.
    let mut pak = std::fs::read(dev.build.join("data/big.pak")).unwrap();
    pak.splice(150_000..150_000, b"-- inserted in v3 --".iter().copied());
    std::fs::write(dev.build.join("data/big.pak"), &pak).unwrap();
    std::fs::remove_file(dev.build.join("data/words.txt")).unwrap();
    std::fs::write(dev.build.join("data/words2.txt"), "apple\nbanana\ncherry\n").unwrap();
    dev.release("3.0.0").await;
    let update = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert!(
        update.downloaded_bytes < (PAK_SIZE / 5) as u64,
        "downloaded {} bytes for a 20-byte insertion",
        update.downloaded_bytes
    );
    assert!(update.harvested_bytes > (PAK_SIZE / 2) as u64, "{update:?}");
    assert_eq!(update.files_deleted, 1);
    assert!(!install_dir.join("data/words.txt").exists());
    assert_eq!(
        std::fs::read(install_dir.join("data/big.pak")).unwrap(),
        pak
    );
    // The game notices its data is incomplete now (words.txt is gone).
    let code = headless::run_from(headless_args(
        &data_dir,
        &["launch", "sample-game", "--", "--selftest"],
    ))
    .await
    .unwrap();
    assert_eq!(code, 3);

    // Uninstall keeps files the game did not ship and the pin (unless --forget).
    std::fs::write(install_dir.join("savegame.dat"), b"progress").unwrap();
    launcher.uninstall("sample-game", false).await.unwrap();
    assert!(!install_dir.join(exe_name()).exists());
    assert!(install_dir.join("savegame.dat").exists());
    assert!(launcher.find("sample-game").unwrap().installed.is_none());
    launcher.uninstall("sample-game", true).await.unwrap();
    assert!(launcher.find("sample-game").is_err());
}

#[tokio::test]
async fn verify_and_repair() {
    let server = TestServer::start().await;
    let dev = Dev::new(&server, "sample-game", "dev@example.com").await;
    dev.release("1.0.0").await;
    let player = tempfile::tempdir().unwrap();
    let launcher = Launcher::open(player.path().join("data")).unwrap();
    launcher
        .update_settings(|s| s.install_root = player.path().join("games"))
        .unwrap();
    launcher
        .add_game(&format!("{}/games/sample-game", server.url), false)
        .await
        .unwrap();
    let report = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    let dir = report.dir.clone();
    assert_eq!(dir, player.path().join("games/sample-game"));

    // The installed size is recorded, and survives a restart.
    let size = launcher
        .find("sample-game")
        .unwrap()
        .installed
        .unwrap()
        .size;
    assert!(size > PAK_SIZE as u64, "{size}");
    assert_eq!(
        Launcher::open(player.path().join("data"))
            .unwrap()
            .find("sample-game")
            .unwrap()
            .installed
            .unwrap()
            .size,
        size
    );

    // Verify reports its progress in bytes, ending exactly at the total.
    let (progress, seen) = recorder();
    let clean = launcher
        .verify("sample-game", true, progress)
        .await
        .unwrap();
    assert!(clean.bad_files.is_empty() && !clean.repaired);
    assert_eq!(clean.checked_files, 4);
    {
        let seen = seen.lock().unwrap();
        assert!(seen.len() > 2, "{seen:?}");
        assert!(seen
            .iter()
            .all(|p| p.phase == Phase::Verify && p.total == size));
        assert_eq!(seen.iter().map(|p| p.done).max(), Some(size));
    }

    // Flip one byte in the big file, delete another file.
    let mut pak = std::fs::read(dir.join("data/big.pak")).unwrap();
    pak[123_456] ^= 0xff;
    std::fs::write(dir.join("data/big.pak"), &pak).unwrap();
    std::fs::remove_file(dir.join("data/words.txt")).unwrap();

    let found = launcher
        .verify("sample-game", false, no_progress())
        .await
        .unwrap();
    let mut bad = found.bad_files.clone();
    bad.sort();
    assert_eq!(bad, ["data/big.pak", "data/words.txt"]);
    assert!(!found.repaired);
    // Headless: exit code 2 when damage is found and not repaired.
    let code = headless::run_from(headless_args(
        launcher.data_dir(),
        &["verify", "sample-game", "--no-repair"],
    ))
    .await
    .unwrap();
    assert_eq!(code, 2);

    let fixed = launcher
        .verify("sample-game", true, no_progress())
        .await
        .unwrap();
    assert!(fixed.repaired);
    // Only the damaged chunk and the missing file were re-downloaded.
    assert!(
        fixed.downloaded_bytes < 40_000,
        "{}",
        fixed.downloaded_bytes
    );
    assert_same_tree(&dev.build, &dir);
    assert!(launcher
        .verify("sample-game", false, no_progress())
        .await
        .unwrap()
        .bad_files
        .is_empty());
}

/// A server cannot tamper with a game: corrupted chunks, a swapped developer
/// key and a rolled-back channel are all refused.
#[tokio::test]
async fn server_cannot_tamper() {
    let server = TestServer::start().await;
    let dev = Dev::new(&server, "sample-game", "dev@example.com").await;
    dev.release("1.0.0").await;
    let url = format!("{}/games/sample-game", server.url);
    let player = tempfile::tempdir().unwrap();
    let launcher = Launcher::open(player.path().join("data")).unwrap();
    launcher
        .update_settings(|s| s.install_root = player.path().join("games"))
        .unwrap();
    let entry = launcher.add_game(&url, false).await.unwrap();
    let dir = player.path().join("games/sample-game");

    // 1. The server swaps the bytes of one chunk on disk.
    let client = gamedist_core::client::ApiClient::connect(&server.url)
        .await
        .unwrap();
    let release = client
        .release("sample-game", "1.0.0", &entry.pinned_key)
        .await
        .unwrap();
    let victim = release
        .manifest()
        .platforms
        .values()
        .next()
        .unwrap()
        .file("data/big.pak")
        .unwrap()
        .chunks[3];
    let path = server.state.chunks.path(&victim.id);
    let original = std::fs::read(&path).unwrap();
    let mut evil = original.clone();
    evil[0] ^= 1;
    std::fs::write(&path, &evil).unwrap();
    let err = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("failed verification"), "{err}");
    assert!(
        !dir.join(exe_name()).exists(),
        "nothing is installed from a tampered download"
    );
    assert!(launcher.find("sample-game").unwrap().installed.is_none());
    std::fs::write(&path, &original).unwrap();
    launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_same_tree(&dev.build, &dir);

    // 2. The server (or whoever took it over) replaces the game with one
    //    signed by a different key under the same slug.
    let attacker = Dev::new(&server, "evil-twin", "attacker@example.com").await;
    attacker.release("9.0.0").await;
    let attacker_key = commands::key_public(&attacker.ctx, "evil-twin").unwrap();
    sqlx::query("UPDATE games SET slug = 'sample-game-old' WHERE slug = 'sample-game'")
        .execute(&server.state.db)
        .await
        .unwrap();
    // The attacker's game must be signed for the slug it is served under.
    let hijack = Dev::new(&server, "sample-game", "attacker2@example.com").await;
    hijack.release("9.0.0").await;
    let hijack_key = commands::key_public(&hijack.ctx, "sample-game").unwrap();
    assert_ne!(hijack_key, entry.pinned_key);
    let _ = attacker_key;

    for attempt in ["check", "install", "add"] {
        let err = match attempt {
            "check" => launcher
                .check_update("sample-game")
                .await
                .map(|_| ())
                .unwrap_err(),
            "install" => launcher
                .install("sample-game", InstallOptions::default(), no_progress())
                .await
                .map(|_| ())
                .unwrap_err(),
            _ => launcher
                .add_game(&url, false)
                .await
                .map(|_| ())
                .unwrap_err(),
        };
        match err {
            LauncherError::KeyChanged {
                pinned, presented, ..
            } => {
                assert_eq!(pinned, entry.pinned_key.to_string());
                assert_eq!(presented, hijack_key.to_string());
            }
            other => panic!("{attempt}: expected KeyChanged, got {other}"),
        }
    }
    // Still the original game on disk, still the original pin.
    assert_same_tree(&dev.build, &dir);
    let still = launcher.find("sample-game").unwrap();
    assert_eq!(still.pinned_key, entry.pinned_key);
    assert_eq!(still.installed.as_ref().unwrap().version, "1.0.0");
    // Headless refuses too, with a non-zero exit.
    assert!(headless::run_from(headless_args(
        launcher.data_dir(),
        &["update", "sample-game"]
    ))
    .await
    .is_err());

    // Only an explicit decision re-pins.
    let rekeyed = launcher.add_game(&url, true).await.unwrap();
    assert_eq!(rekeyed.pinned_key, hijack_key);
    assert_ne!(rekeyed.game_id, entry.game_id);
    assert_eq!(launcher.games().len(), 1);
    let report = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_eq!(report.version, "9.0.0");
}

#[tokio::test]
async fn downgrade_protection_and_channels() {
    let server = TestServer::start().await;
    let dev = Dev::new(&server, "sample-game", "dev@example.com").await;
    dev.release("1.0.0").await;
    dev.release("2.0.0").await;
    let player = tempfile::tempdir().unwrap();
    let launcher = Launcher::open(player.path().join("data")).unwrap();
    launcher
        .update_settings(|s| s.install_root = player.path().join("games"))
        .unwrap();
    launcher
        .add_game(&format!("{}/games/sample-game", server.url), false)
        .await
        .unwrap();
    let report = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_eq!(report.version, "2.0.0");

    // The (unsigned) channel pointer is moved back to an older signed release.
    commands::promote(&dev.ctx, "1.0.0", "stable", None)
        .await
        .unwrap();
    let err = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("refusing to downgrade"), "{err}");
    assert_eq!(
        launcher
            .find("sample-game")
            .unwrap()
            .installed
            .unwrap()
            .version,
        "2.0.0"
    );
    let report = launcher
        .install(
            "sample-game",
            InstallOptions {
                allow_downgrade: true,
                ..Default::default()
            },
            no_progress(),
        )
        .await
        .unwrap();
    assert_eq!(report.version, "1.0.0");
    assert_eq!(
        std::fs::read_to_string(report.dir.join("data/version.txt")).unwrap(),
        "1.0.0\n"
    );

    // Following another channel and pinning an exact version.
    commands::promote(&dev.ctx, "2.0.0", "beta", None)
        .await
        .unwrap();
    let report = launcher
        .install(
            "sample-game",
            InstallOptions {
                channel: Some("beta".into()),
                ..Default::default()
            },
            no_progress(),
        )
        .await
        .unwrap();
    assert_eq!(report.version, "2.0.0");
    assert_eq!(launcher.find("sample-game").unwrap().channel, "beta");
    let err = launcher
        .install(
            "sample-game",
            InstallOptions {
                channel: Some("nightly".into()),
                ..Default::default()
            },
            no_progress(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("no release on the \"nightly\" channel"),
        "{err}"
    );
    let err = launcher
        .install(
            "sample-game",
            InstallOptions {
                platform: Some("plan9-mips".into()),
                ..Default::default()
            },
            no_progress(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no plan9-mips build"), "{err}");

    // An install directory in use by something else is refused.
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("unrelated.txt"), b"x").unwrap();
    let launcher2 = Launcher::open(player.path().join("data2")).unwrap();
    launcher2
        .add_game(&format!("{}/games/sample-game", server.url), false)
        .await
        .unwrap();
    let err = launcher2
        .install(
            "sample-game",
            InstallOptions {
                dir: Some(other.path().to_path_buf()),
                ..Default::default()
            },
            no_progress(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not empty"), "{err}");
    assert!(other.path().join("unrelated.txt").exists());
}

/// Phase 5 acceptance: the same game installed from two different hosts
/// shares Game ID, pinned key and content store; a signed key rotation is
/// followed without user interaction.
#[tokio::test]
async fn mirrors_share_identity_and_rotation_is_followed() {
    let mut origin = TestServer::start().await;
    let mirror = TestServer::start().await;
    let dev = Dev::new(&origin, "sample-game", "dev@example.com").await;
    dev.release("1.0.0").await;
    login(&dev.ctx, &mirror.url, "dev@example.com").await;
    commands::mirror(&dev.ctx, &mirror.url, None).await.unwrap();

    let player = tempfile::tempdir().unwrap();
    let launcher = Launcher::open(player.path().join("data")).unwrap();
    launcher
        .update_settings(|s| {
            s.install_root = player.path().join("games");
            s.keep_chunks = true;
        })
        .unwrap();
    let from_origin = launcher
        .add_game(&format!("{}/games/sample-game", origin.url), false)
        .await
        .unwrap();
    let first = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert!(first.downloaded_bytes > PAK_SIZE as u64);

    // Adding the mirror URL does not create a second game.
    let from_mirror = launcher
        .add_game(&format!("{}/games/sample-game", mirror.url), false)
        .await
        .unwrap();
    assert_eq!(from_mirror.game_id, from_origin.game_id);
    assert_eq!(from_mirror.pinned_key, from_origin.pinned_key);
    assert_eq!(
        from_mirror.sources,
        [origin.url.clone(), mirror.url.clone()]
    );
    assert_eq!(launcher.games().len(), 1);

    // Origin goes away. Reinstalling works through the mirror, and because
    // the content store is shared (chunk IDs are host-independent) nothing
    // is downloaded again.
    launcher.uninstall("sample-game", false).await.unwrap();
    origin.shutdown().await;
    let again = launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_eq!(
        again.downloaded_bytes, 0,
        "chunks came from the shared store"
    );
    assert_same_tree(&dev.build, &again.dir);

    // A second player who only knows the mirror gets the same identity.
    let launcher_b = Launcher::open(player.path().join("data-b")).unwrap();
    launcher_b
        .update_settings(|s| s.install_root = player.path().join("games-b"))
        .unwrap();
    let b = launcher_b
        .add_game(&format!("{}/games/sample-game", mirror.url), false)
        .await
        .unwrap();
    assert_eq!(
        (b.game_id.clone(), b.pinned_key),
        (from_origin.game_id.clone(), from_origin.pinned_key)
    );
    launcher_b
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();

    // Key rotation on the remaining server, then a release under the new key.
    let mut project = gamedist_cli::config::Project::load(&dev.ctx.project_dir).unwrap();
    project.servers = vec![mirror.url.clone()];
    project.save(&dev.ctx.project_dir).unwrap();
    let (new_key, _) = commands::key_rotate(&dev.ctx, "sample-game-2025")
        .await
        .unwrap();
    dev.release("2.0.0").await;

    // The launcher follows the signed statement on its own: no KeyChanged.
    let report = launcher_b
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_eq!(report.version, "2.0.0");
    assert_eq!(report.previous_version.as_deref(), Some("1.0.0"));
    assert_eq!(
        report.downloaded_bytes, 6,
        "still a delta update across the rotation"
    );
    let rotated = launcher_b.find("sample-game").unwrap();
    assert_eq!(rotated.pinned_key, new_key);
    assert_eq!(
        rotated.game_id,
        gamedist_core::game_id(&new_key, "sample-game")
    );
    assert_eq!(launcher_b.games().len(), 1);
    let marker: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report.dir.join("installed.json")).unwrap())
            .unwrap();
    assert_eq!(marker["developer_pubkey"], new_key.to_string());
}

#[tokio::test]
async fn catalogs_discover_and_settings() {
    let server = TestServer::start().await;
    let dev = Dev::new(&server, "sample-game", "dev@example.com").await;
    dev.release("1.0.0").await;
    let player = tempfile::tempdir().unwrap();
    let data_dir = player.path().join("data");
    let launcher = Launcher::open(&data_dir).unwrap();

    assert!(launcher.discover("").await.0.is_empty());
    launcher
        .add_catalog(&format!("{}/", server.url))
        .await
        .unwrap();
    launcher.add_catalog(&server.url).await.unwrap();
    assert_eq!(launcher.catalogs(), std::slice::from_ref(&server.url));
    assert!(launcher.add_catalog("http://127.0.0.1:1").await.is_err());

    let (found, errors) = launcher.discover("sample").await;
    assert!(errors.is_empty());
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].game.url,
        format!("{}/games/sample-game", server.url)
    );
    assert!(!found[0].in_library);
    launcher.add_game(&found[0].game.url, false).await.unwrap();
    assert!(launcher.discover("").await.0[0].in_library);
    assert!(launcher.discover("zzz").await.0.is_empty());

    // State survives a restart; settings via the headless CLI.
    drop(launcher);
    let code = headless::run_from(headless_args(
        &data_dir,
        &[
            "settings",
            "--bandwidth-limit",
            "200000",
            "--concurrency",
            "2",
            "--keep-chunks",
            "true",
            "--transport",
            "direct",
        ],
    ))
    .await
    .unwrap();
    assert_eq!(code, 0);
    let launcher = Launcher::open(&data_dir).unwrap();
    let settings = launcher.settings();
    assert_eq!(
        (
            settings.bandwidth_limit,
            settings.concurrency,
            settings.keep_chunks
        ),
        (Some(200_000), 2, true)
    );
    assert_eq!(launcher.games().len(), 1);
    assert_eq!(launcher.catalogs().len(), 1);

    // The bandwidth limit is honoured: at half the game's size per second
    // the download takes about two seconds.
    let size = std::fs::metadata(dev.build.join(exe_name())).unwrap().len() + PAK_SIZE as u64;
    launcher
        .update_settings(|s| {
            s.install_root = player.path().join("games");
            s.bandwidth_limit = Some(size / 2);
        })
        .unwrap();
    let started = std::time::Instant::now();
    launcher
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(1200),
        "finished in {elapsed:?}"
    );
    assert!(
        !launcher.store().list().unwrap().is_empty(),
        "keep_chunks keeps the store"
    );

    launcher.remove_catalog(&server.url).unwrap();
    assert!(launcher.catalogs().is_empty());
    assert_eq!(
        headless::run_from(headless_args(&data_dir, &["list"]))
            .await
            .unwrap(),
        0
    );
    assert!(
        headless::run_from(headless_args(&data_dir, &["launch", "nope"]))
            .await
            .is_err()
    );
}

async fn wait_for_torrent(server: &TestServer, slug: &str, version: &str) {
    let client = gamedist_core::client::ApiClient::connect(&server.url)
        .await
        .unwrap();
    for _ in 0..400 {
        let list = client.releases(slug).await.unwrap();
        if list
            .releases
            .iter()
            .any(|r| r.version == version && !r.torrents.is_empty())
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("no torrent for {slug} {version}");
}

fn player(
    root: &Path,
    name: &str,
    edit: impl FnOnce(&mut gamedist_launcher::Settings),
) -> Launcher {
    let launcher = Launcher::open(root.join(format!("data-{name}"))).unwrap();
    let games = root.join(format!("games-{name}"));
    launcher
        .update_settings(|s| {
            s.install_root = games;
            s.torrent_stall_secs = 3;
            edit(s)
        })
        .unwrap();
    launcher
}

/// Phase 3 acceptance: a second launcher gets (at least) half of the game
/// from the first launcher over BitTorrent rather than from the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn torrent_swarm_between_two_launchers() {
    use gamedist_launcher::Transport;
    let server = TestServer::start().await;
    let dev = Dev::new(&server, "sample-game", "dev@example.com").await;
    dev.release("1.0.0").await;
    wait_for_torrent(&server, "sample-game", "1.0.0").await;
    let url = format!("{}/games/sample-game", server.url);
    let root = tempfile::tempdir().unwrap();

    // Player A installs over HTTP and seeds.
    let a = player(root.path(), "a", |s| s.seed = true);
    a.add_game(&url, false).await.unwrap();
    let first = a
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert!(first.seeding, "A should be seeding after install");
    assert_eq!(first.torrent_bytes, 0);
    let total: u64 =
        std::fs::metadata(dev.build.join(exe_name())).unwrap().len() + PAK_SIZE as u64 + 6 + 20;
    assert_eq!(a.seeding()[0].total_bytes, total);
    assert!(a.torrent_port().is_some());

    // Player B prefers the swarm.
    let b = player(root.path(), "b", |s| s.transport = Transport::TorrentDirect);
    b.add_game(&url, false).await.unwrap();
    let (progress, seen) = recorder();
    let second = b
        .install("sample-game", InstallOptions::default(), progress)
        .await
        .unwrap();
    {
        // Download progress says where the bytes come from.
        let seen = seen.lock().unwrap();
        let downloads: Vec<_> = seen.iter().filter(|p| p.phase == Phase::Download).collect();
        assert!(
            downloads.iter().all(|p| p.source.is_some()),
            "{downloads:?}"
        );
        assert!(
            downloads
                .iter()
                .any(|p| p.source == Some(Source::Peers) && p.done > 0),
            "no bytes reported from peers: {downloads:?}"
        );
        assert!(seen
            .iter()
            .all(|p| p.phase == Phase::Download || p.source.is_none()));
    }
    assert!(
        second.torrent_bytes * 2 >= total,
        "B got only {} of {total} bytes from the swarm ({second:?})",
        second.torrent_bytes
    );
    assert!(second.downloaded_bytes * 2 <= total, "{second:?}");
    assert_same_tree(&dev.build, &second.dir);
    let uploaded = a.seeding()[0].uploaded_bytes;
    assert!(
        uploaded * 2 >= total,
        "A uploaded only {uploaded} of {total}"
    );
    // Everything B installed was verified against the signed manifest.
    assert!(b
        .verify("sample-game", false, no_progress())
        .await
        .unwrap()
        .bad_files
        .is_empty());

    // Torrent-only works too while a seeder is around.
    let c = player(root.path(), "c", |s| s.transport = Transport::Torrent);
    c.add_game(&url, false).await.unwrap();
    let third = c
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_eq!(
        third.downloaded_bytes, 0,
        "torrent-only must not touch HTTP chunks"
    );
    assert_eq!(third.torrent_bytes, total);
    assert_same_tree(&dev.build, &third.dir);

    // A updates while seeding: seeding stops, files change, seeding resumes
    // with the new version's torrent.
    dev.release("2.0.0").await;
    wait_for_torrent(&server, "sample-game", "2.0.0").await;
    let old_hash = a.seeding()[0].info_hash.clone();
    let update = a
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert!(update.seeding);
    assert_ne!(a.seeding()[0].info_hash, old_hash);
    assert_eq!(a.seeding().len(), 1);

    // A damaged install is not seeded.
    std::fs::write(update.dir.join("data/words.txt"), "tampered\n").unwrap();
    let err = a.seed_game("sample-game").await.unwrap_err();
    assert!(err.to_string().contains("do not match"), "{err}");
    assert!(a.seeding().is_empty());

    // With no seeder left: torrent+direct falls back to HTTP, torrent-only fails.
    a.shutdown().await;
    b.shutdown().await;
    c.shutdown().await;
    let d = player(root.path(), "d", |s| s.transport = Transport::TorrentDirect);
    d.add_game(&url, false).await.unwrap();
    let fallback = d
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap();
    assert_eq!(fallback.torrent_bytes, 0);
    assert_eq!(fallback.downloaded_bytes, total);
    assert_same_tree(&dev.build, &fallback.dir);

    let e = player(root.path(), "e", |s| s.transport = Transport::Torrent);
    e.add_game(&url, false).await.unwrap();
    let err = e
        .install("sample-game", InstallOptions::default(), no_progress())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("torrent only"), "{err}");
    assert!(e.find("sample-game").unwrap().installed.is_none());
    d.shutdown().await;
    e.shutdown().await;
}
