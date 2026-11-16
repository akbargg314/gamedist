//! Integration tests for every REST endpoint, against a real server on an
//! ephemeral port with a temporary SQLite database.

mod common;

use common::*;
use gamedist_core::{
    api::{CreateGameRequest, PatchGameRequest},
    manifest::{Assets, KeyRotation, Registration, SignedRelease},
    schema::{self, Schema},
    torrent::Bencode,
    ChunkId, SecretKey,
};
use serde_json::{json, Value};

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

#[tokio::test]
async fn well_known_matches_schema() {
    let server = spawn(|c| c.name = "Test Server".into()).await;
    let value: Value = http()
        .get(format!("{}/.well-known/gamedist.json", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    schema::validate(Schema::ServerInfo, &value).unwrap();
    assert_eq!(value["name"], "Test Server");
    assert_eq!(value["registration"], "open");
    assert_eq!(value["chunks"], format!("{}/chunks/", server.url));
    assert_eq!(value["tracker"], format!("{}/announce", server.url));
}

#[tokio::test]
async fn registration_and_tokens() {
    let server = spawn(|_| {}).await;
    let anon = server.anonymous().await;
    anon.register("First@Example.com", PASSWORD, None)
        .await
        .unwrap();
    assert_eq!(
        status_of(anon.register("first@example.com", PASSWORD, None).await),
        409
    );
    assert_eq!(
        status_of(anon.register("x@example.com", "short", None).await),
        400
    );
    assert_eq!(
        status_of(anon.register("not-an-email", PASSWORD, None).await),
        400
    );
    assert_eq!(
        status_of(
            anon.token("first@example.com", "wrong password!", None)
                .await
        ),
        401
    );
    assert_eq!(
        status_of(anon.token("nobody@example.com", PASSWORD, None).await),
        401
    );
    assert_eq!(status_of(anon.me().await), 401);

    let token = anon
        .token("first@example.com", PASSWORD, None)
        .await
        .unwrap();
    assert!(token.starts_with("gd_"));
    let first = anon.clone().with_token(token);
    let me = first.me().await.unwrap();
    assert_eq!(me.email, "first@example.com");
    assert!(me.is_admin, "first account on an open server is the admin");

    let second = server.developer("second@example.com").await;
    assert!(!second.me().await.unwrap().is_admin);
    assert_eq!(
        status_of(anon.clone().with_token("gd_bogus").me().await),
        401
    );

    // Problem documents are RFC 7807.
    let resp = http()
        .get(format!("{}/api/v0/dev/me", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let problem: Value = resp.json().await.unwrap();
    assert_eq!(problem["status"], 401);
    assert!(problem["title"].is_string());
}

#[tokio::test]
async fn closed_and_invite_registration() {
    let server = spawn(|c| c.registration = Registration::Closed).await;
    let anon = server.anonymous().await;
    assert_eq!(
        status_of(anon.register("a@example.com", PASSWORD, None).await),
        403
    );

    // Operator bootstraps an admin out of band (the `user-add` subcommand).
    let hash = gamedist_server::auth::hash_password(PASSWORD.into())
        .await
        .unwrap();
    gamedist_server::api::insert_user(&server.state, "root@example.com", &hash, true)
        .await
        .unwrap();
    let admin = anon.clone().with_token(
        anon.token("root@example.com", PASSWORD, None)
            .await
            .unwrap(),
    );

    let settings: Value = admin
        .send_json(
            reqwest::Method::POST,
            "/admin/settings",
            &json!({"registration": "invite"}),
        )
        .await
        .unwrap();
    assert_eq!(settings["registration"], "invite");
    assert_eq!(
        anon.info().registration,
        Registration::Closed,
        "cached info is stale by design"
    );
    assert_eq!(
        server.anonymous().await.info().registration,
        Registration::Invite
    );

    assert_eq!(
        status_of(anon.register("b@example.com", PASSWORD, None).await),
        403
    );
    assert_eq!(
        status_of(anon.register("b@example.com", PASSWORD, Some("nope")).await),
        403
    );
    let invite: Value = admin
        .send_json(reqwest::Method::POST, "/admin/invites", &json!({}))
        .await
        .unwrap();
    let code = invite["code"].as_str().unwrap();
    anon.register("b@example.com", PASSWORD, Some(code))
        .await
        .unwrap();
    assert_eq!(
        status_of(anon.register("c@example.com", PASSWORD, Some(code)).await),
        403,
        "invite codes are single use"
    );
    assert_eq!(
        status_of(
            admin
                .send_json::<_, Value>(
                    reqwest::Method::POST,
                    "/admin/settings",
                    &json!({"registration": "sometimes"})
                )
                .await
        ),
        400
    );
}

#[tokio::test]
async fn publish_flow_end_to_end() {
    let server = spawn(|_| {}).await;
    let dev = server.developer("dev@example.com").await;
    let key = SecretKey::generate();
    create_game(&dev, &key, "cool-game").await;

    // Game manifest (both locations) validates and has no channels yet.
    for url in [
        format!("{}/games/cool-game/manifest.json", server.url),
        format!("{}/api/v0/games/cool-game", server.url),
    ] {
        let value: Value = http().get(url).send().await.unwrap().json().await.unwrap();
        schema::validate(Schema::GameManifest, &value).unwrap();
        assert_eq!(value["channels"], json!({}));
    }
    let game = dev.game("cool-game").await.unwrap();
    assert_eq!(
        game.game_id,
        gamedist_core::game_id(&key.public(), "cool-game")
    );
    assert_eq!(
        game.releases,
        format!("{}/games/cool-game/releases/", server.url)
    );

    // Chunks.
    let (release, chunks) = make_release(
        &key,
        "cool-game",
        "1.0.0",
        &[
            ("game.bin", b"0123456789abcdefXYZ"),
            ("data/a.txt", b"hello world"),
        ],
    );
    let ids: Vec<ChunkId> = chunks.iter().map(|(id, _)| *id).collect();
    assert_eq!(dev.missing_chunks(&ids).await.unwrap(), ids);
    assert!(
        dev.put_chunk(&ids[0], chunks[0].1.clone()).await.unwrap(),
        "201 on first upload"
    );
    assert!(
        !dev.put_chunk(&ids[0], chunks[0].1.clone()).await.unwrap(),
        "204 on re-upload"
    );
    assert_eq!(
        status_of(dev.put_chunk(&ids[1], b"wrong bytes".to_vec()).await),
        422
    );
    assert_eq!(dev.missing_chunks(&ids).await.unwrap(), ids[1..].to_vec());
    assert_eq!(
        status_of(
            server
                .anonymous()
                .await
                .put_chunk(&ids[1], chunks[1].1.clone())
                .await
        ),
        401
    );
    assert_eq!(
        status_of(server.anonymous().await.missing_chunks(&ids).await),
        401
    );

    // Publishing with chunks missing is refused.
    assert_eq!(
        status_of(dev.publish_release("cool-game", &release).await),
        400
    );
    assert!(publish(&dev, "cool-game", &release, &chunks).await);
    assert!(
        !dev.publish_release("cool-game", &release).await.unwrap(),
        "idempotent re-publish"
    );
    assert!(dev.missing_chunks(&ids).await.unwrap().is_empty());

    // Same version, different content: immutable.
    let (other, other_chunks) =
        make_release(&key, "cool-game", "1.0.0", &[("game.bin", b"different")]);
    for (id, data) in &other_chunks {
        dev.put_chunk(id, data.clone()).await.unwrap();
    }
    assert_eq!(
        status_of(dev.publish_release("cool-game", &other).await),
        409
    );

    // Channels.
    assert_eq!(
        status_of(dev.set_channel("cool-game", "stable", "9.9.9").await),
        404
    );
    assert_eq!(
        status_of(dev.set_channel("cool-game", "Bad Channel", "1.0.0").await),
        400
    );
    dev.set_channel("cool-game", "stable", "1.0.0")
        .await
        .unwrap();
    assert_eq!(
        dev.game("cool-game").await.unwrap().channels["stable"],
        "1.0.0"
    );

    // The signed manifest comes back byte-identical and verifies.
    for url in [
        format!("{}/games/cool-game/releases/1.0.0.json", server.url),
        format!("{}/api/v0/games/cool-game/releases/1.0.0", server.url),
    ] {
        let text = http().get(url).send().await.unwrap().text().await.unwrap();
        assert_eq!(text, release.text());
        SignedRelease::parse_pinned(&text, &key.public()).unwrap();
    }
    dev.release("cool-game", "1.0.0", &key.public())
        .await
        .unwrap();
    assert!(dev
        .release("cool-game", "1.0.0", &SecretKey::generate().public())
        .await
        .is_err());
    assert_eq!(status_of(dev.release_text("cool-game", "2.0.0").await), 404);

    // Chunks: immutable caching headers, HEAD, verification.
    let resp = http()
        .get(format!("{}/chunks/{}", server.url, ids[0]))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(resp.bytes().await.unwrap().to_vec(), chunks[0].1);
    let head = http()
        .head(format!("{}/api/v0/chunks/{}", server.url, ids[0]))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(
        head.headers()["content-length"],
        chunks[0].1.len().to_string().as_str()
    );
    for (id, data) in &chunks {
        assert_eq!(&dev.chunk(id).await.unwrap(), data);
    }
    let unknown = ChunkId::of(b"never uploaded");
    assert_eq!(
        http()
            .get(format!("{}/chunks/{unknown}", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http()
            .get(format!("{}/chunks/not-hex", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );

    // Release list, torrent and web seed.
    let torrent_url = wait_for_torrent(&dev, "cool-game", "1.0.0").await;
    assert_eq!(
        torrent_url,
        format!(
            "{}/games/cool-game/releases/1.0.0-linux-x86_64.torrent",
            server.url
        )
    );
    let list = dev.releases("cool-game").await.unwrap();
    assert_eq!(list.channels["stable"], "1.0.0");
    assert_eq!(list.releases[0].platforms, ["linux-x86_64"]);
    let torrent = http()
        .get(&torrent_url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let meta = Bencode::decode(&torrent).unwrap();
    let info = meta.get("info").unwrap();
    assert_eq!(info.get("name").unwrap().as_bytes().unwrap(), b"files");
    assert_eq!(
        meta.get("announce").unwrap().as_bytes().unwrap(),
        format!("{}/announce", server.url).as_bytes()
    );
    let Bencode::List(seeds) = meta.get("url-list").unwrap() else {
        panic!()
    };
    let seed = String::from_utf8(seeds[0].as_bytes().unwrap().to_vec()).unwrap();
    assert_eq!(
        seed,
        format!(
            "{}/games/cool-game/releases/1.0.0/linux-x86_64/",
            server.url
        )
    );
    // Piece hashes cover the concatenated files.
    use sha1::Digest;
    let all = b"0123456789abcdefXYZhello world";
    assert_eq!(
        info.get("pieces").unwrap().as_bytes().unwrap(),
        sha1::Sha1::digest(all).as_slice()
    );

    // BEP 19: <web seed><name>/<path>.
    let file_url = format!("{seed}files/game.bin");
    let resp = http().get(&file_url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["accept-ranges"], "bytes");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"0123456789abcdefXYZ");
    // A range crossing chunk boundaries (chunks are 8 bytes).
    let resp = http()
        .get(&file_url)
        .header("range", "bytes=6-17")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.headers()["content-range"], "bytes 6-17/19");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"6789abcdefXY");
    let resp = http()
        .get(&file_url)
        .header("range", "bytes=-3")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"XYZ");
    let resp = http()
        .get(&file_url)
        .header("range", "bytes=19-")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 416);
    let nested = http()
        .get(format!("{seed}files/data/a.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(nested.bytes().await.unwrap().as_ref(), b"hello world");
    assert_eq!(
        http()
            .get(format!("{seed}files/nope"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http()
            .get(format!("{seed}files/../../x"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // Catalog and export.
    let page = dev.catalog("", 1).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.games[0].slug, "cool-game");
    assert_eq!(page.games[0].url, format!("{}/games/cool-game", server.url));
    assert_eq!(dev.catalog("cool", 1).await.unwrap().total, 1);
    assert_eq!(dev.catalog("nomatch", 1).await.unwrap().total, 0);
    assert_eq!(
        dev.catalog("%", 1).await.unwrap().total,
        0,
        "LIKE wildcards are escaped"
    );
    let export = http()
        .get(format!("{}/api/v0/export.jsonl", server.url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let lines: Vec<&str> = export.lines().collect();
    assert_eq!(lines.len(), 1);
    schema::validate(
        Schema::GameManifest,
        &serde_json::from_str(lines[0]).unwrap(),
    )
    .unwrap();

    // Metadata + assets.
    let icon = ChunkId::of(b"icon png");
    let patch = PatchGameRequest {
        title: Some("Cool Game!".into()),
        assets: Some(Assets {
            icon: Some(icon),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        status_of(dev.patch_game("cool-game", &patch).await),
        400,
        "asset chunk not uploaded"
    );
    dev.put_chunk(&icon, b"icon png".to_vec()).await.unwrap();
    let updated = dev.patch_game("cool-game", &patch).await.unwrap();
    assert_eq!(updated.title, "Cool Game!");
    assert_eq!(updated.assets.icon, Some(icon));
    assert_eq!(updated.summary, "A test game", "untouched fields are kept");

    // Unpublish.
    dev.delete_release("cool-game", "1.0.0").await.unwrap();
    assert_eq!(
        status_of(dev.delete_release("cool-game", "1.0.0").await),
        404
    );
    assert_eq!(status_of(dev.release_text("cool-game", "1.0.0").await), 404);
    assert!(
        dev.game("cool-game").await.unwrap().channels.is_empty(),
        "channel pointer removed"
    );
    assert_eq!(http().get(&file_url).send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn ownership_and_signature_enforcement() {
    let server = spawn(|_| {}).await;
    let owner = server.developer("owner@example.com").await;
    let mallory = server.developer("mallory@example.com").await;
    let key = SecretKey::generate();
    let mallory_key = SecretKey::generate();

    // A key must be registered to the account before a game can use it.
    let req = CreateGameRequest {
        slug: "my-game".into(),
        developer_pubkey: key.public(),
        title: "My Game".into(),
        summary: String::new(),
        description_md: String::new(),
    };
    assert_eq!(status_of(owner.create_game(&req).await), 403);
    create_game(&owner, &key, "my-game").await;
    assert_eq!(status_of(owner.create_game(&req).await), 409, "slug taken");
    let mut bad = req.clone();
    bad.slug = "Bad Slug".into();
    assert_eq!(status_of(owner.create_game(&bad).await), 400);
    assert_eq!(
        status_of(server.anonymous().await.create_game(&req).await),
        401
    );

    let (release, chunks) = make_release(&key, "my-game", "1.0.0", &[("game", b"payload!")]);
    // Someone else cannot publish, patch, promote or delete.
    for (id, data) in &chunks {
        mallory.put_chunk(id, data.clone()).await.unwrap();
    }
    assert_eq!(
        status_of(mallory.publish_release("my-game", &release).await),
        403
    );
    assert_eq!(
        status_of(
            mallory
                .patch_game(
                    "my-game",
                    &PatchGameRequest {
                        title: Some("pwned".into()),
                        ..Default::default()
                    }
                )
                .await
        ),
        403
    );
    assert!(owner.publish_release("my-game", &release).await.unwrap());
    assert_eq!(
        status_of(mallory.set_channel("my-game", "stable", "1.0.0").await),
        403
    );
    assert_eq!(
        status_of(mallory.delete_release("my-game", "1.0.0").await),
        403
    );

    // The owner's *token* is not enough: a release signed by another key is refused.
    let (forged, _) = make_release(&mallory_key, "my-game", "2.0.0", &[("game", b"payload!")]);
    assert_eq!(
        status_of(owner.publish_release("my-game", &forged).await),
        422
    );

    // A manifest for a different slug, or a tampered one, is refused.
    let (other_slug, _) = make_release(&key, "other-game", "2.0.0", &[("game", b"payload!")]);
    assert_eq!(
        status_of(owner.publish_release("my-game", &other_slug).await),
        400
    );
    let tampered = release.text().replace("1.0.0", "3.0.0");
    let resp = http()
        .post(format!("{}/api/v0/games/my-game/releases", server.url))
        .bearer_auth(owner_token(&owner).await)
        .body(tampered)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
    let resp = http()
        .post(format!("{}/api/v0/games/my-game/releases", server.url))
        .bearer_auth(owner_token(&owner).await)
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    assert_eq!(
        status_of(owner.publish_release("no-such-game", &release).await),
        404
    );
}

/// A fresh token for the same account (the client does not expose its own).
async fn owner_token(client: &gamedist_core::client::ApiClient) -> String {
    let me = client.me().await.unwrap();
    client.token(&me.email, PASSWORD, None).await.unwrap()
}

#[tokio::test]
async fn storage_quota_is_enforced() {
    let server = spawn(|c| c.default_quota_bytes = 20).await;
    let admin = server.developer("admin@example.com").await;
    let dev = server.developer("dev@example.com").await;
    let put = |data: &'static [u8]| {
        let dev = dev.clone();
        async move { dev.put_chunk(&ChunkId::of(data), data.to_vec()).await }
    };
    put(b"0123456789").await.unwrap();
    put(b"abcdefghij").await.unwrap();
    assert_eq!(status_of(put(b"x").await), 507);
    assert!(
        !put(b"0123456789").await.unwrap(),
        "re-uploading existing chunks is free"
    );
    let me = dev.me().await.unwrap();
    assert_eq!((me.storage_used, me.storage_quota), (20, Some(20)));

    // Admin raises this user's quota; 0 means unlimited.
    let users: Value = admin.get_json("/admin/users").await.unwrap();
    let id = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"] == "dev@example.com")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let _: Value = admin
        .send_json(
            reqwest::Method::POST,
            "/admin/users",
            &json!({"id": id, "quota_bytes": 0}),
        )
        .await
        .unwrap();
    put(b"x").await.unwrap();
    assert_eq!(dev.me().await.unwrap().storage_quota, None);
    let _: Value = admin
        .send_json(
            reqwest::Method::POST,
            "/admin/users",
            &json!({"id": id, "quota_bytes": -1}),
        )
        .await
        .unwrap();
    assert_eq!(dev.me().await.unwrap().storage_quota, Some(20));

    // Oversized bodies never reach storage.
    let big = vec![0u8; 8 * 1024 * 1024 + 2048];
    let result = admin.put_chunk(&ChunkId::of(&big), big).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn admin_takedown_users_and_audit() {
    let server = spawn(|_| {}).await;
    let admin = server.developer("admin@example.com").await;
    let dev = server.developer("dev@example.com").await;
    let key = SecretKey::generate();
    create_game(&dev, &key, "bad-game").await;
    let (release, chunks) = make_release(&key, "bad-game", "1.0.0", &[("game", b"payload!")]);
    publish(&dev, "bad-game", &release, &chunks).await;
    dev.set_channel("bad-game", "stable", "1.0.0")
        .await
        .unwrap();
    wait_for_torrent(&dev, "bad-game", "1.0.0").await;

    // Admin endpoints are admin-only.
    for path in [
        "/admin/users",
        "/admin/games",
        "/admin/takedowns",
        "/admin/settings",
        "/admin/audit",
    ] {
        assert_eq!(status_of(dev.get_json::<Value>(path).await), 403, "{path}");
        assert_eq!(
            status_of(server.anonymous().await.get_json::<Value>(path).await),
            401,
            "{path}"
        );
        admin.get_json::<Value>(path).await.unwrap();
    }
    assert_eq!(
        status_of(
            dev.send_json::<_, Value>(
                reqwest::Method::POST,
                "/admin/takedowns",
                &json!({"slug": "bad-game", "reason": "x"})
            )
            .await
        ),
        403
    );

    // Takedown needs a reason and hides the game everywhere with 451.
    assert_eq!(
        status_of(
            admin
                .send_json::<_, Value>(
                    reqwest::Method::POST,
                    "/admin/takedowns",
                    &json!({"slug": "bad-game"})
                )
                .await
        ),
        400
    );
    let _: Value = admin
        .send_json(
            reqwest::Method::POST,
            "/admin/takedowns",
            &json!({"slug": "bad-game", "reason": "malware report #12"}),
        )
        .await
        .unwrap();
    for url in [
        "/games/bad-game/manifest.json",
        "/games/bad-game/releases/1.0.0.json",
        "/games/bad-game/releases/1.0.0-linux-x86_64.torrent",
        "/games/bad-game/releases/1.0.0/linux-x86_64/files/game",
        "/games/bad-game/releases/",
        "/api/v0/games/bad-game",
        "/api/v0/games/bad-game/releases",
        "/api/v0/games/bad-game/releases/1.0.0",
        "/games/bad-game",
    ] {
        let resp = http()
            .get(format!("{}{url}", server.url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 451, "{url}");
    }
    assert_eq!(dev.catalog("", 1).await.unwrap().total, 0);
    // The owner cannot keep publishing to it.
    let (v2, chunks2) = make_release(&key, "bad-game", "2.0.0", &[("game", b"payload2")]);
    for (id, data) in &chunks2 {
        dev.put_chunk(id, data.clone()).await.unwrap();
    }
    assert_eq!(status_of(dev.publish_release("bad-game", &v2).await), 451);

    let games: Value = admin.get_json("/admin/games").await.unwrap();
    assert_eq!(games["games"][0]["taken_down"], true);
    assert_eq!(games["games"][0]["takedown_reason"], "malware report #12");
    let takedowns: Value = admin.get_json("/admin/takedowns").await.unwrap();
    assert_eq!(takedowns["takedowns"][0]["reason"], "malware report #12");

    // Restore.
    let _: Value = admin
        .send_json(
            reqwest::Method::POST,
            "/admin/takedowns",
            &json!({"slug": "bad-game", "restore": true}),
        )
        .await
        .unwrap();
    dev.game("bad-game").await.unwrap();

    // Disabling a user kills their tokens.
    let users: Value = admin.get_json("/admin/users").await.unwrap();
    let find = |email: &str| {
        users["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["email"] == email)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    let _: Value = admin
        .send_json(
            reqwest::Method::POST,
            "/admin/users",
            &json!({"id": find("dev@example.com"), "disabled": true}),
        )
        .await
        .unwrap();
    assert_eq!(status_of(dev.me().await), 401);
    assert_eq!(
        status_of(dev.token("dev@example.com", PASSWORD, None).await),
        403
    );
    assert_eq!(
        status_of(
            admin
                .send_json::<_, Value>(
                    reqwest::Method::POST,
                    "/admin/users",
                    &json!({"id": find("admin@example.com"), "disabled": true})
                )
                .await
        ),
        400,
        "admins cannot lock themselves out"
    );

    // Everything above left an audit trail.
    let audit: Value = admin.get_json("/admin/audit").await.unwrap();
    let actions: Vec<&str> = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    for expected in [
        "user.register",
        "game.create",
        "release.publish",
        "channel.set",
        "game.takedown",
        "game.restore",
        "user.update",
    ] {
        assert!(
            actions.contains(&expected),
            "missing {expected} in {actions:?}"
        );
    }
}

#[tokio::test]
async fn tracker_announce() {
    let server = spawn(|_| {}).await;
    let dev = server.developer("dev@example.com").await;
    let key = SecretKey::generate();
    create_game(&dev, &key, "swarm").await;
    let (release, chunks) = make_release(&key, "swarm", "1.0.0", &[("game", b"payload!")]);
    publish(&dev, "swarm", &release, &chunks).await;
    wait_for_torrent(&dev, "swarm", "1.0.0").await;
    let list = dev.releases("swarm").await.unwrap();
    let info_hash = &list.releases[0].torrents["linux-x86_64"].info_hash;
    let encoded: String = data_encoding::HEXLOWER
        .decode(info_hash.as_bytes())
        .unwrap()
        .iter()
        .map(|b| format!("%{b:02x}"))
        .collect();

    let announce = |peer: char, port: u16, left: u64| {
        let url = format!(
            "{}/announce?info_hash={encoded}&peer_id={}&port={port}&left={left}&compact=1",
            server.url,
            peer.to_string().repeat(20)
        );
        async move {
            Bencode::decode(&http().get(url).send().await.unwrap().bytes().await.unwrap()).unwrap()
        }
    };
    let first = announce('a', 7001, 0).await;
    assert_eq!(first.get("complete").unwrap().as_int(), Some(1));
    assert_eq!(first.get("peers").unwrap().as_bytes().unwrap().len(), 0);
    let second = announce('b', 7002, 100).await;
    assert_eq!(second.get("incomplete").unwrap().as_int(), Some(1));
    assert_eq!(
        second.get("peers").unwrap().as_bytes().unwrap(),
        &[127, 0, 0, 1, 0x1b, 0x59]
    );

    // Not an open tracker.
    let foreign = format!(
        "{}/announce?info_hash={}&peer_id={}&port=1&left=0",
        server.url,
        "%aa".repeat(20),
        "c".repeat(20)
    );
    let resp = Bencode::decode(
        &http()
            .get(foreign)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(resp.get("failure reason").is_some());
    let resp = Bencode::decode(
        &http()
            .get(format!("{}/announce", server.url))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(resp.get("failure reason").is_some());
}

#[tokio::test]
async fn garbage_collection_only_removes_unreferenced_chunks() {
    let server = spawn(|_| {}).await;
    let dev = server.developer("dev@example.com").await;
    let key = SecretKey::generate();
    create_game(&dev, &key, "gc-game").await;
    let (v1, chunks1) = make_release(&key, "gc-game", "1.0.0", &[("game", b"version-one-data")]);
    let (v2, chunks2) = make_release(&key, "gc-game", "2.0.0", &[("game", b"version-TWO-data")]);
    publish(&dev, "gc-game", &v1, &chunks1).await;
    publish(&dev, "gc-game", &v2, &chunks2).await;
    let orphan = ChunkId::of(b"orphan");
    dev.put_chunk(&orphan, b"orphan".to_vec()).await.unwrap();
    let icon = ChunkId::of(b"icon");
    dev.put_chunk(&icon, b"icon".to_vec()).await.unwrap();
    dev.patch_game(
        "gc-game",
        &PatchGameRequest {
            assets: Some(Assets {
                icon: Some(icon),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Within the grace period nothing is touched.
    assert_eq!(
        gamedist_server::jobs::collect_garbage(&server.state, 3600)
            .await
            .unwrap(),
        (0, 0)
    );
    // Past it, only the orphan goes.
    assert_eq!(
        gamedist_server::jobs::collect_garbage(&server.state, -1)
            .await
            .unwrap(),
        (1, 6)
    );
    assert!(!server.state.chunks.has(&orphan));
    assert!(server.state.chunks.has(&icon));

    // Unpublishing v1 frees only chunks v2 does not share ("-data" tail is shared).
    dev.delete_release("gc-game", "1.0.0").await.unwrap();
    let (removed, _) = gamedist_server::jobs::collect_garbage(&server.state, -1)
        .await
        .unwrap();
    assert_eq!(removed, 1);
    for (id, _) in &chunks2 {
        assert!(server.state.chunks.has(id));
    }
    dev.release("gc-game", "2.0.0", &key.public())
        .await
        .unwrap();
}

#[tokio::test]
async fn auth_rate_limit() {
    let server = spawn(|c| c.auth_rate_per_minute = 3).await;
    let anon = server.anonymous().await;
    for _ in 0..3 {
        assert_eq!(
            status_of(anon.token("x@example.com", PASSWORD, None).await),
            401
        );
    }
    assert_eq!(
        status_of(anon.token("x@example.com", PASSWORD, None).await),
        429
    );
}

#[tokio::test]
async fn key_rotation() {
    let server = spawn(|_| {}).await;
    let dev = server.developer("dev@example.com").await;
    let old = SecretKey::generate();
    let new = SecretKey::generate();
    create_game(&dev, &old, "rotating").await;
    let (v1, chunks) = make_release(&old, "rotating", "1.0.0", &[("game", b"payload!")]);
    publish(&dev, "rotating", &v1, &chunks).await;
    dev.set_channel("rotating", "stable", "1.0.0")
        .await
        .unwrap();

    let statement = KeyRotation::create(&old, new.public(), "rotating").unwrap();
    assert_eq!(
        status_of(dev.post_rotation("rotating", &statement).await),
        403,
        "new key must be registered"
    );
    dev.add_key(new.public(), None).await.unwrap();
    // A statement signed by the wrong key is refused.
    let forged = KeyRotation::create(&new, old.public(), "rotating").unwrap();
    assert_eq!(status_of(dev.post_rotation("rotating", &forged).await), 400);
    dev.post_rotation("rotating", &statement).await.unwrap();

    let game = dev.game("rotating").await.unwrap();
    assert_eq!(game.developer_pubkey, new.public());
    assert_eq!(
        game.game_id,
        gamedist_core::game_id(&new.public(), "rotating")
    );
    assert_eq!(game.rotations.len(), 1);
    assert!(gamedist_core::manifest::rotation_chain_links(
        "rotating",
        &old.public(),
        &new.public(),
        &game.rotations
    ));
    assert!(game.channels.is_empty());

    // Old key can no longer publish; new key can.
    let (v2_old, c) = make_release(&old, "rotating", "2.0.0", &[("game", b"payload2")]);
    for (id, data) in &c {
        dev.put_chunk(id, data.clone()).await.unwrap();
    }
    assert_eq!(
        status_of(dev.publish_release("rotating", &v2_old).await),
        422
    );
    let (v2_new, _) = make_release(&new, "rotating", "2.0.0", &[("game", b"payload2")]);
    assert!(dev.publish_release("rotating", &v2_new).await.unwrap());
}

#[tokio::test]
async fn metrics_and_catalog_toggle() {
    let server = spawn(|c| c.catalog = false).await;
    let dev = server.developer("dev@example.com").await;
    dev.put_chunk(&ChunkId::of(b"abc"), b"abc".to_vec())
        .await
        .unwrap();
    dev.chunk(&ChunkId::of(b"abc")).await.unwrap();
    let text = http()
        .get(format!("{}/metrics", server.url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains("gamedist_chunks_uploaded_total 1"), "{text}");
    assert!(text.contains("gamedist_chunk_bytes_served_total 3"));
    assert!(text.contains("gamedist_users 1"));
    assert!(text.contains("# TYPE gamedist_chunk_bytes gauge"));

    assert_eq!(status_of(dev.catalog("", 1).await), 404);
    assert_eq!(
        http()
            .get(format!("{}/api/v0/export.jsonl", server.url))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert!(dev.info().catalog.is_none());
}
