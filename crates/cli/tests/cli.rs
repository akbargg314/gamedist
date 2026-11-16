//! CLI commands against real servers on ephemeral ports (Phase 1 acceptance:
//! push, idempotent re-push, fetch and verify everything).

use gamedist_cli::{
    commands::{self, AssetOptions, Ctx, InitOptions, LoginOptions, PushOptions},
    config::Credentials,
};
use gamedist_core::{
    chunker::ChunkParams, client::ApiClient, manifest::Registration, ChunkId, SecretKey,
};
use gamedist_server::{Config, Server};
use std::path::{Path, PathBuf};

const PASSWORD: &str = "correct horse battery";

struct TestServer {
    url: String,
    _tmp: tempfile::TempDir,
    _stop: tokio::sync::oneshot::Sender<()>,
}

async fn server() -> TestServer {
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
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(server.run(async {
        let _ = rx.await;
    }));
    TestServer {
        url,
        _tmp: tmp,
        _stop: tx,
    }
}

fn ctx(root: &Path) -> Ctx {
    let config_dir = root.join("config");
    Ctx {
        project_dir: root.join("project"),
        credentials: Credentials::file_only(&config_dir),
        config_dir,
        chunk_params: ChunkParams::small(),
        verbose: false,
    }
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

fn write_build(dir: &Path, version_marker: &str) {
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(
        dir.join("game.sh"),
        format!("#!/bin/sh\necho {version_marker}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("data/big.pak"), pseudo_random(200_000, 42)).unwrap();
    std::fs::write(dir.join("data/small.txt"), b"same in every version").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("game.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
}

async fn login(ctx: &Ctx, server: &str, email: &str, register: bool) {
    commands::login(
        ctx,
        LoginOptions {
            server: server.to_string(),
            email: email.to_string(),
            password: PASSWORD.to_string(),
            register,
            invite: None,
        },
    )
    .await
    .unwrap();
}

fn push_opts(dir: PathBuf, version: &str) -> PushOptions {
    PushOptions {
        builds: vec![("linux-x86_64".into(), dir)],
        version: version.into(),
        channel: Some("stable".into()),
        changelog: Some(format!("Release {version}")),
        server: None,
        exe: Some("game.sh".into()),
        concurrency: 4,
    }
}

#[tokio::test]
async fn init_login_create_push_promote_verify() {
    let srv = server().await;
    let tmp = tempfile::tempdir().unwrap();
    let ctx = ctx(tmp.path());

    // init
    let (project, pubkey) = commands::init(
        &ctx,
        InitOptions {
            slug: "sample-game".into(),
            title: Some("Sample Game".into()),
            key: None,
            server: Some(format!("{}/", srv.url)),
        },
    )
    .unwrap();
    assert_eq!(project.servers, std::slice::from_ref(&srv.url));
    assert_eq!(commands::key_public(&ctx, "sample-game").unwrap(), pubkey);
    assert!(
        commands::init(
            &ctx,
            InitOptions {
                slug: "sample-game".into(),
                title: None,
                key: None,
                server: None
            }
        )
        .is_err(),
        "init refuses to overwrite"
    );

    // Not logged in yet.
    let err = commands::game_create(&ctx, None, "", "").await.unwrap_err();
    assert!(err.to_string().contains("gamedist login"), "{err}");

    login(&ctx, &srv.url, "dev@example.com", true).await;
    let games = commands::game_create(&ctx, None, "A tiny game", "# Sample")
        .await
        .unwrap();
    assert_eq!(
        games[0].game_id,
        gamedist_core::game_id(&pubkey, "sample-game")
    );

    // push v1
    let build = tmp.path().join("build");
    write_build(&build, "v1");
    let report = commands::push(&ctx, push_opts(build.clone(), "1.0.0"))
        .await
        .unwrap();
    assert_eq!(report.len(), 1);
    assert!(report[0].created);
    assert!(report[0].chunks_total > 10, "{report:?}");
    assert_eq!(report[0].chunks_uploaded, report[0].chunks_total);
    assert_eq!(report[0].bytes_uploaded, 200_000 + 18 + 21);

    // Idempotent: pushing the same build again uploads nothing.
    let again = commands::push(&ctx, push_opts(build.clone(), "1.0.0"))
        .await
        .unwrap();
    assert_eq!(again[0].chunks_uploaded, 0);
    assert_eq!(again[0].bytes_uploaded, 0);
    assert!(!again[0].created);

    // The launch exe from --exe was remembered in gamedist.toml.
    let saved = gamedist_cli::config::Project::load(&ctx.project_dir).unwrap();
    assert_eq!(saved.platforms["linux-x86_64"].launch.exe, "game.sh");

    // Fetch like `curl` would: manifest verifies, every chunk verifies and
    // reassembles into the original files.
    let client = ApiClient::connect(&srv.url).await.unwrap();
    let game = client.game("sample-game").await.unwrap();
    assert_eq!(game.channels["stable"], "1.0.0");
    assert_eq!(game.title, "Sample Game");
    let release = client
        .release("sample-game", "1.0.0", &pubkey)
        .await
        .unwrap();
    assert_eq!(release.manifest().changelog_md, "Release 1.0.0");
    let platform = &release.manifest().platforms["linux-x86_64"];
    assert_eq!(platform.launch.exe, "game.sh");
    for file in &platform.files {
        let mut assembled = Vec::new();
        for chunk in &file.chunks {
            let data = reqwest::get(client.info().chunk_url(&chunk.id))
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            assert_eq!(ChunkId::of(&data), chunk.id);
            assembled.extend_from_slice(&data);
        }
        assert_eq!(
            assembled,
            std::fs::read(build.join(&file.path)).unwrap(),
            "{}",
            file.path
        );
    }
    let script = platform.file("game.sh").unwrap();
    assert_eq!(script.mode, 0o755);

    // Changing content under an existing version is refused.
    write_build(&build, "v1-changed");
    let err = commands::push(&ctx, push_opts(build.clone(), "1.0.0"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("immutable"), "{err}");

    // v2 changes one small file: only its chunk is uploaded (delta push).
    write_build(&build, "v2");
    let mut opts = push_opts(build.clone(), "2.0.0");
    opts.channel = Some("beta".into());
    let v2 = commands::push(&ctx, opts).await.unwrap();
    assert_eq!(v2[0].chunks_uploaded, 1);
    assert_eq!(v2[0].bytes_uploaded, 18);
    let game = client.game("sample-game").await.unwrap();
    assert_eq!(
        (
            game.channels["stable"].as_str(),
            game.channels["beta"].as_str()
        ),
        ("1.0.0", "2.0.0")
    );

    // promote
    commands::promote(&ctx, "2.0.0", "stable", None)
        .await
        .unwrap();
    assert_eq!(
        client.game("sample-game").await.unwrap().channels["stable"],
        "2.0.0"
    );
    assert!(commands::promote(&ctx, "9.9.9", "stable", None)
        .await
        .is_err());

    // status
    let status = commands::status(&ctx).await.unwrap();
    assert_eq!(status.pubkey, pubkey);
    assert!(status.servers[0].logged_in);
    let list = status.servers[0].releases.as_ref().unwrap();
    assert_eq!(list.releases.len(), 2);

    // verify (offline)
    let file = tmp.path().join("release.json");
    std::fs::write(&file, release.text()).unwrap();
    let verified = commands::verify(&file, Some(&pubkey)).unwrap();
    assert_eq!(verified.version, "1.0.0");
    assert!(commands::verify(&file, Some(&SecretKey::generate().public())).is_err());
    std::fs::write(&file, release.text().replace("game.sh", "evil.sh")).unwrap();
    assert!(commands::verify(&file, None).is_err());

    // Bad inputs.
    assert!(
        commands::push(&ctx, push_opts(build.clone(), "not-a-version"))
            .await
            .is_err()
    );
    assert!(
        commands::push(&ctx, push_opts(tmp.path().join("missing"), "3.0.0"))
            .await
            .is_err()
    );
    let mut no_exe = push_opts(build.clone(), "3.0.0");
    no_exe.builds = vec![("windows-x86_64".into(), build.clone())];
    no_exe.exe = None;
    let err = commands::push(&ctx, no_exe).await.unwrap_err();
    assert!(err.to_string().contains("launch executable"), "{err}");
}

#[tokio::test]
async fn assets_keys_and_multi_platform() {
    let srv = server().await;
    let tmp = tempfile::tempdir().unwrap();
    let ctx = ctx(tmp.path());
    commands::init(
        &ctx,
        InitOptions {
            slug: "multi".into(),
            title: None,
            key: Some("studio".into()),
            server: Some(srv.url.clone()),
        },
    )
    .unwrap();
    login(&ctx, &srv.url, "dev@example.com", true).await;
    commands::game_create(&ctx, None, "", "").await.unwrap();

    // Key export/import round trip.
    let exported = commands::key_export(&ctx, "studio").unwrap();
    assert!(exported.starts_with("ed25519-secret:"));
    let imported = commands::key_import(&ctx, "backup", &exported).unwrap();
    assert_eq!(imported, commands::key_public(&ctx, "studio").unwrap());
    assert!(
        commands::key_import(&ctx, "backup", &exported).is_err(),
        "no overwrite"
    );
    assert!(commands::key_import(&ctx, "bad", "garbage").is_err());

    // Assets.
    let icon = tmp.path().join("icon.png");
    let shot = tmp.path().join("shot.png");
    std::fs::write(&icon, b"\x89PNG icon").unwrap();
    std::fs::write(&shot, b"\x89PNG shot").unwrap();
    let assets = commands::assets_set(
        &ctx,
        AssetOptions {
            icon: Some(icon.clone()),
            screenshots: Some(vec![shot.clone()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(assets.icon, Some(ChunkId::of(b"\x89PNG icon")));
    assert_eq!(assets.screenshots, [ChunkId::of(b"\x89PNG shot")]);
    // Setting only the banner keeps the rest.
    let assets = commands::assets_set(
        &ctx,
        AssetOptions {
            banner: Some(shot),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(assets.icon.is_some() && assets.banner.is_some() && assets.screenshots.len() == 1);
    let empty = tmp.path().join("empty.png");
    std::fs::write(&empty, b"").unwrap();
    assert!(commands::assets_set(
        &ctx,
        AssetOptions {
            icon: Some(empty),
            ..Default::default()
        }
    )
    .await
    .is_err());

    // One release, two platforms, sharing data chunks.
    let linux = tmp.path().join("linux");
    let windows = tmp.path().join("windows");
    write_build(&linux, "v1");
    write_build(&windows, "v1");
    std::fs::rename(windows.join("game.sh"), windows.join("game.exe")).unwrap();
    let mut project = gamedist_cli::config::Project::load(&ctx.project_dir).unwrap();
    for (platform, exe) in [("linux-x86_64", "game.sh"), ("windows-x86_64", "game.exe")] {
        project.platforms.insert(
            platform.into(),
            gamedist_cli::config::PlatformConfig {
                launch: gamedist_cli::config::LaunchConfig {
                    exe: exe.into(),
                    args: vec!["--windowed".into()],
                    cwd: None,
                },
            },
        );
    }
    project.save(&ctx.project_dir).unwrap();
    let report = commands::push(
        &ctx,
        PushOptions {
            builds: vec![
                ("linux-x86_64".into(), linux),
                ("windows-x86_64".into(), windows),
            ],
            version: "1.0.0".into(),
            channel: None,
            changelog: None,
            server: None,
            exe: None,
            concurrency: 2,
        },
    )
    .await
    .unwrap();
    assert!(report[0].created);
    let client = ApiClient::connect(&srv.url).await.unwrap();
    let pubkey = commands::key_public(&ctx, "studio").unwrap();
    let release = client.release("multi", "1.0.0", &pubkey).await.unwrap();
    let m = release.manifest();
    assert_eq!(m.platforms.len(), 2);
    assert_eq!(m.platforms["windows-x86_64"].launch.args, ["--windowed"]);
    // Shared content is stored once.
    let per_platform: usize = m.platforms.values().map(|p| p.unique_chunks().len()).sum();
    assert!(m.unique_chunks().len() < per_platform);
    assert!(
        client.game("multi").await.unwrap().channels.is_empty(),
        "no channel unless asked"
    );
}

#[tokio::test]
async fn mirror_to_second_server_and_rotate_key() {
    let origin = server().await;
    let mirror = server().await;
    let tmp = tempfile::tempdir().unwrap();
    let ctx = ctx(tmp.path());
    commands::init(
        &ctx,
        InitOptions {
            slug: "mirrored".into(),
            title: Some("Mirrored".into()),
            key: None,
            server: Some(origin.url.clone()),
        },
    )
    .unwrap();
    login(&ctx, &origin.url, "dev@example.com", true).await;
    commands::game_create(&ctx, None, "summary", "desc")
        .await
        .unwrap();
    let icon = tmp.path().join("icon.png");
    std::fs::write(&icon, b"icon bytes").unwrap();
    commands::assets_set(
        &ctx,
        AssetOptions {
            icon: Some(icon),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let build = tmp.path().join("build");
    write_build(&build, "v1");
    commands::push(&ctx, push_opts(build.clone(), "1.0.0"))
        .await
        .unwrap();
    write_build(&build, "v2");
    commands::push(&ctx, push_opts(build.clone(), "2.0.0"))
        .await
        .unwrap();

    // Mirroring requires an account on the target.
    assert!(commands::mirror(&ctx, &mirror.url, None).await.is_err());
    login(&ctx, &mirror.url, "mirror-op@example.com", true).await;
    let report = commands::mirror(&ctx, &mirror.url, None).await.unwrap();
    assert_eq!(report.versions, ["1.0.0", "2.0.0"]);
    assert!(report.chunks_copied > 10);

    let a = ApiClient::connect(&origin.url).await.unwrap();
    let b = ApiClient::connect(&mirror.url).await.unwrap();
    let (ga, gb) = (
        a.game("mirrored").await.unwrap(),
        b.game("mirrored").await.unwrap(),
    );
    // Same identity on both hosts.
    assert_eq!(ga.game_id, gb.game_id);
    assert_eq!(ga.developer_pubkey, gb.developer_pubkey);
    assert_eq!(ga.channels, gb.channels);
    assert_eq!(ga.assets, gb.assets);
    assert_eq!(
        (gb.title.as_str(), gb.summary.as_str()),
        ("Mirrored", "summary")
    );
    assert_ne!(ga.releases, gb.releases);
    // Byte-identical signed manifests.
    for version in ["1.0.0", "2.0.0"] {
        assert_eq!(
            a.release_text("mirrored", version).await.unwrap(),
            b.release_text("mirrored", version).await.unwrap()
        );
    }
    // Mirroring again copies nothing.
    let again = commands::mirror(&ctx, &mirror.url, Some("2.0.0"))
        .await
        .unwrap();
    assert_eq!((again.chunks_copied, again.bytes_copied), (0, 0));
    // The mirror was added to the project, so a later push goes to both.
    let project = gamedist_cli::config::Project::load(&ctx.project_dir).unwrap();
    assert_eq!(project.servers.len(), 2);
    write_build(&build, "v3");
    let reports = commands::push(&ctx, push_opts(build.clone(), "3.0.0"))
        .await
        .unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(
        a.release_text("mirrored", "3.0.0").await.unwrap(),
        b.release_text("mirrored", "3.0.0").await.unwrap(),
        "one signature, published to every server"
    );

    // Key rotation: both servers move to the new key and carry the statement.
    let old = commands::key_public(&ctx, "mirrored").unwrap();
    let (new, statement) = commands::key_rotate(&ctx, "mirrored-2").await.unwrap();
    assert_ne!(old, new);
    gamedist_core::manifest::KeyRotation::verify(&statement).unwrap();
    for client in [&a, &b] {
        let game = client.game("mirrored").await.unwrap();
        assert_eq!(game.developer_pubkey, new);
        assert!(gamedist_core::manifest::rotation_chain_links(
            "mirrored",
            &old,
            &new,
            &game.rotations
        ));
    }
    assert_eq!(
        gamedist_cli::config::Project::load(&ctx.project_dir)
            .unwrap()
            .key,
        "mirrored-2"
    );
    // Publishing continues under the new key.
    write_build(&build, "v4");
    let reports = commands::push(&ctx, push_opts(build, "4.0.0"))
        .await
        .unwrap();
    assert!(reports.iter().all(|r| r.created));
    a.release("mirrored", "4.0.0", &new).await.unwrap();
}
