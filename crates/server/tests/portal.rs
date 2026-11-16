//! Web portal tests: pages render, sessions and CSRF work, admin is gated.

mod common;

use common::*;
use gamedist_core::SecretKey;

/// A browser-ish client: keeps cookies manually, does not follow redirects.
struct Browser {
    http: reqwest::Client,
    base: String,
    cookie: Option<String>,
}

impl Browser {
    fn new(base: &str) -> Self {
        Browser {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            base: base.to_string(),
            cookie: None,
        }
    }

    async fn get(&self, path: &str) -> (u16, String) {
        let mut req = self.http.get(format!("{}{path}", self.base));
        if let Some(cookie) = &self.cookie {
            req = req.header("cookie", cookie);
        }
        let resp = req.send().await.unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }

    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> (u16, String, String) {
        let mut req = self.http.post(format!("{}{path}", self.base)).form(form);
        if let Some(cookie) = &self.cookie {
            req = req.header("cookie", cookie);
        }
        let resp = req.send().await.unwrap();
        if let Some(set) = resp.headers().get("set-cookie") {
            let pair = set.to_str().unwrap().split(';').next().unwrap().to_string();
            self.cookie = (!pair.ends_with('=')).then_some(pair);
        }
        let location = resp
            .headers()
            .get("location")
            .map(|l| l.to_str().unwrap().to_string())
            .unwrap_or_default();
        (resp.status().as_u16(), location, resp.text().await.unwrap())
    }

    async fn csrf(&self, page: &str) -> String {
        let (_, html) = self.get(page).await;
        let marker = "name=\"csrf\" value=\"";
        let start = html.find(marker).expect("page has a csrf field") + marker.len();
        html[start..start + 32].to_string()
    }
}

#[tokio::test]
async fn public_pages_render() {
    let server = spawn(|c| c.name = "Portal Test".into()).await;
    let dev = server.developer("dev@example.com").await;
    let key = SecretKey::generate();
    create_game(&dev, &key, "page-game").await;
    let (release, chunks) = make_release(&key, "page-game", "1.0.0", &[("game", b"payload!")]);
    publish(&dev, "page-game", &release, &chunks).await;
    dev.set_channel("page-game", "stable", "1.0.0")
        .await
        .unwrap();
    dev.patch_game(
        "page-game",
        &gamedist_core::api::PatchGameRequest {
            description_md: Some("Hello <script>alert(1)</script> **bold**".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let browser = Browser::new(&server.url);
    let (status, html) = browser.get("/").await;
    assert_eq!(status, 200);
    assert!(html.contains("Portal Test") && html.contains("Game page-game"));
    assert!(html.contains("href=\"/games/page-game\""));
    let (_, html) = browser.get("/?q=nomatch").await;
    assert!(!html.contains("Game page-game"));

    let resp = reqwest::get(format!("{}/games/page-game", server.url))
        .await
        .unwrap();
    assert!(resp.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("default-src 'self'"));
    assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    let html = resp.text().await.unwrap();
    assert!(html.contains("Latest release: 1.0.0"));
    assert!(html.contains(&format!(
        "gamedist+http://{}/games/page-game",
        server.url.trim_start_matches("http://")
    )));
    assert!(html.contains(&key.public().to_string()));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(
        !html.contains("<script>alert(1)</script>"),
        "markdown HTML must be escaped"
    );
    assert!(html.contains("Changes in 1.0.0"));

    assert_eq!(browser.get("/games/nope").await.0, 404);
    let (status, css) = browser.get("/static/style.css").await;
    assert_eq!(status, 200);
    assert!(css.contains("--accent"));
}

#[tokio::test]
async fn session_dashboard_and_csrf() {
    let server = spawn(|_| {}).await;
    let mut browser = Browser::new(&server.url);

    // Not logged in: dashboard redirects to login.
    let resp = browser
        .http
        .get(format!("{}/dashboard", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    assert_eq!(resp.headers()["location"], "/login");
    assert_eq!(browser.get("/login").await.0, 200);

    // Register through the portal (first user: admin), which logs in.
    let (status, location, _) = browser
        .post(
            "/register",
            &[("email", "admin@example.com"), ("password", PASSWORD)],
        )
        .await;
    assert_eq!((status, location.as_str()), (303, "/dashboard"));
    let cookie = browser.cookie.clone().expect("session cookie set");
    assert!(cookie.starts_with("gd_session=gd_"));
    let (status, html) = browser.get("/dashboard").await;
    assert_eq!(status, 200);
    assert!(html.contains("Developer dashboard") && html.contains("admin@example.com"));

    // Wrong password shows an error and no session.
    let mut other = Browser::new(&server.url);
    let (status, _, html) = other
        .post(
            "/login",
            &[
                ("email", "admin@example.com"),
                ("password", "wrong password"),
            ],
        )
        .await;
    assert_eq!(status, 401);
    assert!(html.contains("wrong email or password"));
    assert!(other.cookie.is_none());

    // CSRF: a state-changing POST without the token is refused.
    let key = SecretKey::generate();
    let pubkey = key.public().to_string();
    let (status, _, _) = browser
        .post(
            "/dashboard/keys",
            &[("csrf", "0".repeat(32).as_str()), ("pubkey", &pubkey)],
        )
        .await;
    assert_eq!(status, 403);
    // A session cookie is not an API token and vice versa.
    let token = cookie.trim_start_matches("gd_session=");
    let api = server.anonymous().await.with_token(token);
    assert_eq!(status_of(api.me().await), 401);

    // With the token: add key, create game, edit it, create an API token.
    let csrf = browser.csrf("/dashboard").await;
    let (status, location, _) = browser
        .post(
            "/dashboard/keys",
            &[("csrf", &csrf), ("pubkey", &pubkey), ("label", "laptop")],
        )
        .await;
    assert_eq!(status, 303, "{location}");
    let (status, location, _) = browser
        .post(
            "/dashboard/games",
            &[
                ("csrf", &csrf),
                ("slug", "web-game"),
                ("title", "Web Game"),
                ("pubkey", &pubkey),
            ],
        )
        .await;
    assert_eq!(
        (status, location.as_str()),
        (303, "/dashboard/games/web-game")
    );
    let (status, _, _) = browser
        .post(
            "/dashboard/games/web-game",
            &[
                ("csrf", &csrf),
                ("title", "Web Game 2"),
                ("summary", "New summary"),
                ("description_md", "# Hi"),
            ],
        )
        .await;
    assert_eq!(status, 303);
    let (_, html) = browser.get("/dashboard/games/web-game").await;
    assert!(html.contains("Web Game 2") && html.contains("New summary"));

    let (status, _, html) = browser
        .post("/dashboard/tokens", &[("csrf", &csrf), ("name", "ci")])
        .await;
    assert_eq!(status, 200);
    let start = html.find("<code class=\"token\">").unwrap() + 20;
    let api_token = &html[start..start + html[start..].find('<').unwrap()];
    let api = server.anonymous().await.with_token(api_token);
    assert_eq!(api.me().await.unwrap().games, ["web-game"]);

    // Publish via API, then promote and unpublish from the dashboard.
    let (release, chunks) = make_release(&key, "web-game", "1.0.0", &[("game", b"payload!")]);
    publish(&api, "web-game", &release, &chunks).await;
    let (status, _, _) = browser
        .post(
            "/dashboard/games/web-game/channel",
            &[("csrf", &csrf), ("version", "1.0.0"), ("channel", "beta")],
        )
        .await;
    assert_eq!(status, 303);
    assert_eq!(
        api.game("web-game").await.unwrap().channels["beta"],
        "1.0.0"
    );
    let (status, _, _) = browser
        .post(
            "/dashboard/games/web-game/unpublish",
            &[("csrf", &csrf), ("version", "1.0.0")],
        )
        .await;
    assert_eq!(status, 303);
    assert!(api.releases("web-game").await.unwrap().releases.is_empty());

    // Admin page works for the admin: settings, invite, takedown.
    let (status, html) = browser.get("/admin").await;
    assert_eq!(status, 200);
    assert!(html.contains("Audit log") && html.contains("web-game"));
    let (status, _, _) = browser
        .post(
            "/admin/settings",
            &[
                ("csrf", &csrf),
                ("registration", "invite"),
                ("default_quota_mib", "5"),
            ],
        )
        .await;
    assert_eq!(status, 303);
    assert_eq!(
        server.anonymous().await.info().registration,
        gamedist_core::manifest::Registration::Invite
    );
    assert_eq!(api.me().await.unwrap().storage_quota, Some(5 * 1024 * 1024));
    let (_, location, _) = browser.post("/admin/invites", &[("csrf", &csrf)]).await;
    assert!(location.contains("Invite+code"), "{location}");
    let (status, _, _) = browser
        .post(
            "/admin/takedown",
            &[("csrf", &csrf), ("slug", "web-game"), ("reason", "dmca")],
        )
        .await;
    assert_eq!(status, 303);
    assert_eq!(browser.get("/games/web-game").await.0, 451);

    // A second, non-admin user cannot see /admin or someone else's game.
    let invite_code = location.rsplit("%3A+").next().unwrap().to_string();
    let mut second = Browser::new(&server.url);
    let (status, _, _) = second
        .post(
            "/register",
            &[
                ("email", "dev@example.com"),
                ("password", PASSWORD),
                ("invite", &invite_code),
            ],
        )
        .await;
    assert_eq!(status, 303);
    assert_eq!(second.get("/admin").await.0, 403);
    assert_eq!(second.get("/dashboard/games/web-game").await.0, 403);

    // Logout revokes the session server-side.
    let (status, _, _) = browser.post("/logout", &[("csrf", &csrf)]).await;
    assert_eq!(status, 303);
    let mut stale = Browser::new(&server.url);
    stale.cookie = Some(cookie);
    let resp = stale
        .http
        .get(format!("{}/dashboard", server.url))
        .header("cookie", stale.cookie.as_ref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
}
