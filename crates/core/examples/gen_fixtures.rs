//! Regenerates `protocol/fixtures/*.json` with a fixed, public test key
//! (the RFC 8032 test-vector-1 seed). Run from the repo root:
//! `cargo run -p gamedist-core --example gen_fixtures`.

use gamedist_core::{
    game_id,
    manifest::{ChunkRef, FileEntry, KeyRotation, Launch, Platform, ReleaseManifest},
    ChunkId, SecretKey,
};
use serde_json::{json, Value};
use std::path::Path;

const SEED_B64: &str = "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A";

fn write(dir: &Path, name: &str, value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap() + "\n";
    std::fs::write(dir.join(name), text).unwrap();
}

fn main() {
    let dir = Path::new("protocol/fixtures");
    let key = SecretKey::from_encoded(&format!("ed25519-secret:{SEED_B64}")).unwrap();
    let pk = key.public();
    let chunk = |data: &[u8]| ChunkRef {
        id: ChunkId::of(data),
        size: data.len() as u64,
    };

    let mut release = ReleaseManifest::new(pk, "cool-game", "1.2.0");
    release.created = "2026-11-16T07:00:00Z".into();
    release.changelog_md = "First public release.".into();
    for (platform, exe) in [
        ("linux-x86_64", "CoolGame.x86_64"),
        ("windows-x86_64", "CoolGame.exe"),
    ] {
        let exe_data = format!("binary for {platform}");
        let files = vec![
            FileEntry {
                path: exe.into(),
                size: exe_data.len() as u64,
                mode: 0o755,
                chunks: vec![chunk(exe_data.as_bytes())],
            },
            FileEntry {
                path: "data/levels.pak".into(),
                size: 11,
                mode: 0o644,
                chunks: vec![chunk(b"level"), chunk(b"-data!")],
            },
        ];
        release.platforms.insert(
            platform.into(),
            Platform {
                launch: Launch {
                    exe: exe.into(),
                    args: vec![],
                    cwd: ".".into(),
                },
                total_size: files.iter().map(|f| f.size).sum(),
                files,
            },
        );
    }
    let signed = release.sign(&key).unwrap();
    let release_value: Value = serde_json::from_str(signed.text()).unwrap();
    write(dir, "release-manifest.json", &release_value);

    let server = json!({
        "protocol": "gamedist/0",
        "name": "Example Distributor",
        "api": "https://host.example/api/v0",
        "chunks": "https://cdn.example/chunks/",
        "tracker": "https://host.example/announce",
        "catalog": "https://host.example/api/v0/games",
        "registration": "open"
    });
    write(dir, "server-info.json", &server);

    let game = json!({
        "protocol": "gamedist/0",
        "game_id": game_id(&pk, "cool-game"),
        "slug": "cool-game",
        "developer_pubkey": pk.to_string(),
        "title": "Cool Game",
        "summary": "One-line pitch",
        "description_md": "A **cool** game.",
        "assets": { "icon": ChunkId::of(b"icon").to_hex(), "screenshots": [ChunkId::of(b"shot").to_hex()] },
        "channels": { "stable": "1.2.0", "beta": "1.3.0-beta.1" },
        "releases": "https://host.example/games/cool-game/releases/"
    });
    write(dir, "game-manifest.json", &game);

    // Rotation to a second fixed key (seed = 32 bytes of 0x01).
    let new_key =
        SecretKey::from_encoded("ed25519-secret:AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE")
            .unwrap();
    let mut rotation = KeyRotation::create(&key, new_key.public(), "cool-game").unwrap();
    rotation["created"] = json!("2027-01-01T00:00:00Z");
    gamedist_core::keys::sign_object(&key, &mut rotation).unwrap();
    write(dir, "key-rotation.json", &rotation);

    // Invalid variants (schema violations).
    let inv = dir.join("invalid");
    let mutate = |base: &Value, f: &dyn Fn(&mut Value)| {
        let mut v = base.clone();
        f(&mut v);
        v
    };
    write(
        &inv,
        "server-info-bad-registration.json",
        &mutate(&server, &|v| v["registration"] = json!("maybe")),
    );
    write(
        &inv,
        "server-info-chunks-no-slash.json",
        &mutate(&server, &|v| {
            v["chunks"] = json!("https://cdn.example/chunks")
        }),
    );
    write(
        &inv,
        "game-manifest-bad-slug.json",
        &mutate(&game, &|v| v["slug"] = json!("Cool Game")),
    );
    write(
        &inv,
        "game-manifest-missing-pubkey.json",
        &mutate(&game, &|v| {
            v.as_object_mut().unwrap().remove("developer_pubkey");
        }),
    );
    write(
        &inv,
        "release-manifest-wrong-protocol.json",
        &mutate(&release_value, &|v| v["protocol"] = json!("gamedist/9")),
    );
    write(
        &inv,
        "release-manifest-no-sig.json",
        &mutate(&release_value, &|v| {
            v.as_object_mut().unwrap().remove("sig");
        }),
    );
    write(
        &inv,
        "release-manifest-float-size.json",
        &mutate(&release_value, &|v| {
            v["platforms"]["linux-x86_64"]["total_size"] = json!(1.5)
        }),
    );
    write(
        &inv,
        "release-manifest-huge-chunk.json",
        &mutate(&release_value, &|v| {
            v["platforms"]["linux-x86_64"]["files"][0]["chunks"][0]["size"] = json!(8388609)
        }),
    );
    write(
        &inv,
        "release-manifest-zstd-encoding.json",
        &mutate(&release_value, &|v| {
            v["platforms"]["linux-x86_64"]["files"][0]["chunks"][0]["encoding"] = json!("zstd")
        }),
    );
    write(
        &inv,
        "release-manifest-no-platforms.json",
        &mutate(&release_value, &|v| v["platforms"] = json!({})),
    );
    write(
        &inv,
        "key-rotation-wrong-type.json",
        &mutate(&rotation, &|v| v["type"] = json!("something")),
    );
    println!("fixtures written; game_id = {}", game_id(&pk, "cool-game"));
}
