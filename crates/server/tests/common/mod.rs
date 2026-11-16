//! Shared helpers for server integration tests.
#![allow(dead_code)]

use gamedist_core::{
    api::CreateGameRequest,
    client::ApiClient,
    manifest::{ChunkRef, FileEntry, Launch, Platform, ReleaseManifest, SignedRelease},
    ChunkId, SecretKey,
};
use gamedist_server::{state::St, Config, Server};

pub struct TestServer {
    pub url: String,
    pub state: St,
    _tmp: tempfile::TempDir,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

pub const PASSWORD: &str = "correct horse battery";

/// Start a server on an ephemeral port with open registration.
pub async fn spawn(edit: impl FnOnce(&mut Config)) -> TestServer {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: tmp.path().to_path_buf(),
        registration: gamedist_core::manifest::Registration::Open,
        auth_rate_per_minute: 10_000,
        ..Config::default()
    };
    edit(&mut cfg);
    let server = Server::bind(cfg).await.unwrap();
    let url = server.url().to_string();
    let state = server.state.clone();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
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
        _shutdown: tx,
    }
}

impl TestServer {
    /// Register an account and return a client holding its API token.
    pub async fn developer(&self, email: &str) -> ApiClient {
        let client = ApiClient::connect(&self.url).await.unwrap();
        client.register(email, PASSWORD, None).await.unwrap();
        let token = client.token(email, PASSWORD, Some("test")).await.unwrap();
        client.with_token(token)
    }

    pub async fn anonymous(&self) -> ApiClient {
        ApiClient::connect(&self.url).await.unwrap()
    }
}

/// Register `key` and create a game with it.
pub async fn create_game(client: &ApiClient, key: &SecretKey, slug: &str) {
    client
        .add_key(key.public(), Some("test key"))
        .await
        .unwrap();
    client
        .create_game(&CreateGameRequest {
            slug: slug.to_string(),
            developer_pubkey: key.public(),
            title: format!("Game {slug}"),
            summary: "A test game".into(),
            description_md: "Some **markdown**.".into(),
        })
        .await
        .unwrap();
}

/// Build a one-platform release from in-memory files (8-byte chunks).
/// Returns the signed release and every chunk's bytes.
pub fn make_release(
    key: &SecretKey,
    slug: &str,
    version: &str,
    files: &[(&str, &[u8])],
) -> (SignedRelease, Vec<(ChunkId, Vec<u8>)>) {
    let mut chunks = Vec::new();
    let entries: Vec<FileEntry> = files
        .iter()
        .map(|(path, data)| FileEntry {
            path: path.to_string(),
            size: data.len() as u64,
            mode: 0o755,
            chunks: data
                .chunks(8)
                .map(|c| {
                    let id = ChunkId::of(c);
                    chunks.push((id, c.to_vec()));
                    ChunkRef {
                        id,
                        size: c.len() as u64,
                    }
                })
                .collect(),
        })
        .collect();
    let mut manifest = ReleaseManifest::new(key.public(), slug, version);
    manifest.changelog_md = format!("Changes in {version}");
    manifest.platforms.insert(
        "linux-x86_64".into(),
        Platform {
            launch: Launch {
                exe: files[0].0.to_string(),
                args: vec![],
                cwd: ".".into(),
            },
            total_size: entries.iter().map(|f| f.size).sum(),
            files: entries,
        },
    );
    (manifest.sign(key).unwrap(), chunks)
}

/// Upload chunks and publish.
pub async fn publish(
    client: &ApiClient,
    slug: &str,
    release: &SignedRelease,
    chunks: &[(ChunkId, Vec<u8>)],
) -> bool {
    for (id, data) in chunks {
        client.put_chunk(id, data.clone()).await.unwrap();
    }
    client.publish_release(slug, release).await.unwrap()
}

/// Torrents are generated in the background after publish; wait for them.
pub async fn wait_for_torrent(client: &ApiClient, slug: &str, version: &str) -> String {
    for _ in 0..200 {
        let list = client.releases(slug).await.unwrap();
        if let Some(t) = list
            .releases
            .iter()
            .find(|r| r.version == version)
            .and_then(|r| r.torrents.get("linux-x86_64"))
        {
            return t.url.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("torrent for {slug} {version} never appeared");
}

pub fn status_of<T: std::fmt::Debug>(result: gamedist_core::Result<T>) -> u16 {
    match result {
        Err(gamedist_core::Error::Api { status, .. }) => status,
        other => panic!("expected an API error, got {other:?}"),
    }
}
