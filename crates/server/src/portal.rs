//! Server-rendered web portal: landing page, public game pages, developer
//! dashboard and admin pages.

use crate::{
    api,
    auth::{self, ClientIp, Session, User, SESSION_COOKIE},
    db::{self, GameRow},
    error::ApiError,
    games,
    state::St,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use gamedist_core::manifest::{Assets, Registration, ReleaseManifest};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use sqlx::Row;

const STYLE: &str = include_str!("portal.css");

pub fn routes() -> Router<St> {
    Router::new()
        .route("/", get(landing))
        .route("/static/style.css", get(style))
        .route("/games/{slug}", get(game_page))
        .route("/login", get(login_form).post(login_submit))
        .route("/register", get(register_form).post(register_submit))
        .route("/logout", post(logout))
        .route("/dashboard", get(dashboard))
        .route("/dashboard/keys", post(dashboard_add_key))
        .route("/dashboard/tokens", post(dashboard_create_token))
        .route("/dashboard/tokens/revoke", post(dashboard_revoke_token))
        .route("/dashboard/games", post(dashboard_create_game))
        .route(
            "/dashboard/games/{slug}",
            get(dashboard_game).post(dashboard_update_game),
        )
        .route(
            "/dashboard/games/{slug}/channel",
            post(dashboard_set_channel),
        )
        .route(
            "/dashboard/games/{slug}/unpublish",
            post(dashboard_unpublish),
        )
        .route("/admin", get(admin))
        .route("/admin/users", post(admin_update_user))
        .route("/admin/takedown", post(admin_takedown))
        .route("/admin/settings", post(admin_settings))
        .route("/admin/invites", post(admin_invite))
}

// ---- plumbing --------------------------------------------------------------

enum PortalError {
    Login,
    Api(ApiError),
}

impl From<ApiError> for PortalError {
    fn from(e: ApiError) -> Self {
        PortalError::Api(e)
    }
}

impl From<sqlx::Error> for PortalError {
    fn from(e: sqlx::Error) -> Self {
        PortalError::Api(e.into())
    }
}

impl IntoResponse for PortalError {
    fn into_response(self) -> Response {
        match self {
            PortalError::Login => Redirect::to("/login").into_response(),
            PortalError::Api(e) => {
                let body = html! {
                    section.narrow {
                        h1 { (e.status.as_u16()) " — " (e.title) }
                        @if !e.detail.is_empty() { p { (e.detail) } }
                        p { a href="/" { "Back to the front page" } }
                    }
                };
                let mut response = page_raw("Error", "this server", None, "", body);
                *response.status_mut() = e.status;
                response
            }
        }
    }
}

type PortalResult = Result<Response, PortalError>;

fn require_user(session: &Session) -> Result<&User, PortalError> {
    session.user.as_ref().ok_or(PortalError::Login)
}

fn require_admin(session: &Session) -> Result<&User, PortalError> {
    let user = require_user(session)?;
    if !user.is_admin {
        return Err(ApiError::forbidden("administrator access required").into());
    }
    Ok(user)
}

fn page(st: &St, session: &Session, title: &str, body: Markup) -> Response {
    page_raw(
        title,
        &st.cfg.name,
        session.user.as_ref(),
        &session.csrf,
        body,
    )
}

fn page_raw(title: &str, server: &str, user: Option<&User>, csrf: &str, body: Markup) -> Response {
    let markup = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · " (server) }
                link rel="stylesheet" href="/static/style.css";
            }
            body {
                header.top {
                    a.brand href="/" { (server) }
                    nav {
                        a href="/" { "Games" }
                        @if let Some(user) = user {
                            a href="/dashboard" { "Dashboard" }
                            @if user.is_admin { a href="/admin" { "Admin" } }
                            form.inline method="post" action="/logout" {
                                input type="hidden" name="csrf" value=(csrf);
                                button.link type="submit" { "Log out (" (user.email) ")" }
                            }
                        } @else {
                            a href="/login" { "Log in" }
                        }
                    }
                }
                main { (body) }
                footer {
                    "Powered by " a href="https://github.com/akbargg314/gamedist" { "GameDist" }
                    " — open, decentralized game distribution. Games are signed by their developers; this server cannot alter them."
                }
            }
        }
    };
    let mut response = (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        markup.into_string(),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' https: data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    response
}

async fn style() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        STYLE,
    )
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn date(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

/// Render untrusted Markdown: raw HTML is shown as text and only http(s) and
/// mailto links survive.
pub fn render_markdown(source: &str) -> Markup {
    use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};
    fn safe(url: &CowStr) -> bool {
        let lower = url.trim().to_ascii_lowercase();
        lower.starts_with("https://")
            || lower.starts_with("http://")
            || lower.starts_with("mailto:")
    }
    let parser = Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    )
    .map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) if !safe(&dest_url) => Event::Start(Tag::Link {
            link_type,
            dest_url: "#".into(),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) if !dest_url.trim().to_ascii_lowercase().starts_with("https://") => {
            Event::Start(Tag::Image {
                link_type,
                dest_url: "".into(),
                title,
                id,
            })
        }
        other => other,
    });
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, parser);
    PreEscaped(out)
}

fn install_uri(st: &St, slug: &str) -> String {
    match st.cfg.public_url.split_once("://") {
        Some(("http", rest)) => format!("gamedist+http://{rest}/games/{slug}"),
        Some((_, rest)) => format!("gamedist://{rest}/games/{slug}"),
        None => st.game_url(slug),
    }
}

fn chunk_src(st: &St, id: &gamedist_core::ChunkId) -> String {
    format!("{}{}", st.cfg.chunks_base(), id)
}

fn back(to: &str, message: &str) -> Response {
    let encoded: String = message
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect();
    Redirect::to(&format!("{to}?msg={encoded}")).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct Msg {
    #[serde(default)]
    msg: String,
}

fn flash(msg: &Msg) -> Markup {
    html! { @if !msg.msg.is_empty() { p.flash { (msg.msg) } } }
}

// ---- public pages ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LandingQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    page: Option<u32>,
}

async fn landing(
    State(st): State<St>,
    session: Session,
    Query(query): Query<LandingQuery>,
) -> PortalResult {
    let page_no = query.page.unwrap_or(1).max(1);
    let (rows, total) = if st.cfg.catalog {
        games::catalog(&st, &query.q, page_no).await?
    } else {
        (Vec::new(), 0)
    };
    let pages = (total.div_ceil(games::PAGE_SIZE as u64) as u32).max(1);
    let body = html! {
        section.hero {
            h1 { (st.cfg.name) }
            p { "Paste a game URL from this server into the GameDist launcher to install it. "
                "Every release is signed by its developer, and downloads are verified chunk by chunk." }
        }
        @if st.cfg.catalog {
            form.search method="get" action="/" {
                input type="search" name="q" value=(query.q) placeholder="Search games";
                button type="submit" { "Search" }
            }
            @if rows.is_empty() {
                p.muted { "No games " @if query.q.is_empty() { "have been published yet." } @else { "match your search." } }
            }
            div.grid {
                @for game in &rows {
                    @let assets: Assets = serde_json::from_str(&game.assets_json).unwrap_or_default();
                    a.card href={ "/games/" (game.slug) } {
                        @if let Some(banner) = assets.banner.or(assets.icon) {
                            img src=(chunk_src(&st, &banner)) alt="" loading="lazy";
                        } @else {
                            div.placeholder { (game.title.chars().next().unwrap_or('?')) }
                        }
                        h3 { (game.title) }
                        p { (game.summary) }
                    }
                }
            }
            @if pages > 1 {
                nav.pager {
                    @if page_no > 1 { a href={ "/?q=" (query.q) "&page=" (page_no - 1) } { "← Newer" } }
                    span { "Page " (page_no) " of " (pages) }
                    @if page_no < pages { a href={ "/?q=" (query.q) "&page=" (page_no + 1) } { "Older →" } }
                }
            }
        } @else {
            p.muted { "This server does not list its games publicly. Use a direct game URL." }
        }
    };
    Ok(page(&st, &session, "Games", body))
}

async fn game_page(
    State(st): State<St>,
    session: Session,
    Path(slug): Path<String>,
) -> PortalResult {
    let game = games::public_game(&st, &slug).await?;
    let assets: Assets = serde_json::from_str(&game.assets_json).unwrap_or_default();
    let list = games::release_list(&st, &game).await?;
    let stable = list
        .channels
        .get("stable")
        .or_else(|| list.channels.values().next())
        .cloned();
    let latest: Option<ReleaseManifest> = match &stable {
        Some(version) => games::release_text(&st, &game, version)
            .await
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok()),
        None => None,
    };
    let url = st.game_url(&slug);
    let body = html! {
        article.game {
            @if let Some(banner) = &assets.banner {
                img.banner src=(chunk_src(&st, banner)) alt="";
            }
            div.title {
                @if let Some(icon) = &assets.icon { img.icon src=(chunk_src(&st, icon)) alt=""; }
                div {
                    h1 { (game.title) }
                    p.summary { (game.summary) }
                }
            }
            div.install {
                a.button href=(install_uri(&st, &slug)) { "Install with GameDist" }
                p { "or paste this URL into the launcher:" }
                code { (url) }
            }
            @if let Some(release) = &latest {
                h2 { "Latest release: " (release.version) }
                table {
                    thead { tr { th { "Platform" } th { "Size" } th { "Files" } th { "Torrent" } } }
                    tbody {
                        @for (name, platform) in &release.platforms {
                            tr {
                                td { (name) }
                                td { (human_bytes(platform.total_size)) }
                                td { (platform.files.len()) }
                                td {
                                    @if let Some(t) = list.releases.iter().find(|r| r.version == release.version).and_then(|r| r.torrents.get(name)) {
                                        a href=(t.url) { ".torrent" }
                                    } @else { span.muted { "—" } }
                                }
                            }
                        }
                    }
                }
                @if !release.changelog_md.is_empty() {
                    h3 { "What's new" }
                    div.markdown { (render_markdown(&release.changelog_md)) }
                }
            } @else {
                p.muted { "No release has been published to a channel yet." }
            }
            @if !game.description_md.is_empty() {
                h2 { "About" }
                div.markdown { (render_markdown(&game.description_md)) }
            }
            @if !assets.screenshots.is_empty() {
                h2 { "Screenshots" }
                div.shots {
                    @for shot in &assets.screenshots { img src=(chunk_src(&st, shot)) alt="Screenshot" loading="lazy"; }
                }
            }
            h2 { "Channels and versions" }
            ul {
                @for (channel, version) in &list.channels { li { strong { (channel) } " → " (version) } }
            }
            details {
                summary { "All releases (" (list.releases.len()) ")" }
                ul { @for r in &list.releases { li { (r.version) " — " (r.created) " — " (r.platforms.join(", ")) } } }
            }
            h2 { "Verification" }
            p { "Game ID " code { (game.game_id) } }
            p { "Developer key " code { (game.developer_pubkey) } }
            p.muted { "The launcher pins this key on first install and refuses releases signed by any other key." }
        }
    };
    Ok(page(&st, &session, &game.title, body))
}

// ---- login / registration --------------------------------------------------

fn session_cookie(st: &St, token: &str, max_age: i64) -> HeaderValue {
    let secure = if st.cfg.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn auth_form(action: &str, title: &str, error: Option<&str>, invite: bool, alt: Markup) -> Markup {
    html! {
        section.narrow {
            h1 { (title) }
            @if let Some(error) = error { p.error { (error) } }
            form method="post" action=(action) {
                label { "Email" input type="email" name="email" required autocomplete="username"; }
                label { "Password" input type="password" name="password" required minlength="10"
                    autocomplete=(if action == "/login" { "current-password" } else { "new-password" }); }
                @if invite {
                    label { "Invite code" input type="text" name="invite" required; }
                }
                button type="submit" { (title) }
            }
            (alt)
        }
    }
}

async fn login_form(State(st): State<St>, session: Session) -> PortalResult {
    if session.user.is_some() {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    let alt = html! { p { "No account? " a href="/register" { "Register" } } };
    Ok(page(
        &st,
        &session,
        "Log in",
        auth_form("/login", "Log in", None, false, alt),
    ))
}

#[derive(Debug, Deserialize)]
struct Credentials {
    email: String,
    password: String,
    #[serde(default)]
    invite: Option<String>,
}

async fn start_session(st: &St, user: &User) -> PortalResult {
    let token = auth::create_token(&st.db, user.id, "web session", "session").await?;
    let mut response = Redirect::to("/dashboard").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(st, &token, 30 * 24 * 3600),
    );
    Ok(response)
}

async fn login_submit(
    State(st): State<St>,
    session: Session,
    ClientIp(ip): ClientIp,
    Form(form): Form<Credentials>,
) -> PortalResult {
    st.auth_limiter.check(ip)?;
    match api::login(&st, &form.email, &form.password).await {
        Ok(user) => start_session(&st, &user).await,
        Err(e) => {
            let alt = html! { p { "No account? " a href="/register" { "Register" } } };
            let mut response = page(
                &st,
                &session,
                "Log in",
                auth_form("/login", "Log in", Some(&e.detail), false, alt),
            );
            *response.status_mut() = e.status;
            Ok(response)
        }
    }
}

async fn register_form(State(st): State<St>, session: Session) -> PortalResult {
    let mode = db::registration_mode(&st.db, st.cfg.registration).await?;
    let body = if mode == Registration::Closed {
        html! { section.narrow { h1 { "Registration is closed" }
        p { "This server does not accept new developer accounts. Ask the operator, or run your own server." } } }
    } else {
        auth_form(
            "/register",
            "Register",
            None,
            mode == Registration::Invite,
            html! { p { "Already registered? " a href="/login" { "Log in" } } },
        )
    };
    Ok(page(&st, &session, "Register", body))
}

async fn register_submit(
    State(st): State<St>,
    session: Session,
    ClientIp(ip): ClientIp,
    Form(form): Form<Credentials>,
) -> PortalResult {
    st.auth_limiter.check(ip)?;
    let mode = db::registration_mode(&st.db, st.cfg.registration).await?;
    match api::register_account(&st, &form.email, &form.password, form.invite.as_deref()).await {
        Ok(_) => {
            let user = api::login(&st, &form.email, &form.password).await?;
            start_session(&st, &user).await
        }
        Err(e) => {
            let alt = html! { p { "Already registered? " a href="/login" { "Log in" } } };
            let mut response = page(
                &st,
                &session,
                "Register",
                auth_form(
                    "/register",
                    "Register",
                    Some(&e.detail),
                    mode == Registration::Invite,
                    alt,
                ),
            );
            *response.status_mut() = e.status;
            Ok(response)
        }
    }
}

#[derive(Debug, Deserialize)]
struct CsrfOnly {
    csrf: String,
}

async fn logout(
    State(st): State<St>,
    session: Session,
    Form(form): Form<CsrfOnly>,
) -> PortalResult {
    session.check_csrf(&form.csrf)?;
    if let Some(token) = session.token() {
        auth::revoke_token(&st.db, token).await?;
    }
    let mut response = Redirect::to("/").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&st, "", 0));
    Ok(response)
}

// ---- developer dashboard ---------------------------------------------------

async fn dashboard_body(
    st: &St,
    session: &Session,
    user: &User,
    msg: &Msg,
    new_token: Option<&str>,
) -> Result<Markup, PortalError> {
    let games: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT g.slug, g.title, (SELECT COUNT(*) FROM releases r WHERE r.game_id = g.id)
         FROM games g WHERE g.owner_id = ? ORDER BY g.slug",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let keys: Vec<(String, String)> =
        sqlx::query_as("SELECT pubkey, label FROM dev_keys WHERE user_id = ? ORDER BY id")
            .bind(user.id)
            .fetch_all(&st.db)
            .await?;
    let tokens: Vec<(i64, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, name, created_at, last_used_at FROM tokens WHERE user_id = ? AND kind = 'api' ORDER BY id DESC",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let used = db::storage_used(&st.db, user.id).await?;
    let quota = db::quota_for(&st.db, user.id, st.cfg.default_quota_bytes).await?;
    let csrf = &session.csrf;
    Ok(html! {
        h1 { "Developer dashboard" }
        (flash(msg))
        @if let Some(token) = new_token {
            div.flash {
                p { strong { "New API token" } " — copy it now, it will not be shown again:" }
                code.token { (token) }
            }
        }
        p { "Storage used: " strong { (human_bytes(used)) }
            @if let Some(q) = quota { " of " (human_bytes(q)) } @else { " (no quota)" } }

        h2 { "Your games" }
        @if games.is_empty() { p.muted { "No games yet. Register a signing key below, then create one here or with " code { "gamedist game create" } "." } }
        table {
            @for (slug, title, releases) in &games {
                tr {
                    td { a href={ "/dashboard/games/" (slug) } { (title) } }
                    td { code { (slug) } }
                    td { (releases) " release(s)" }
                    td { a href={ "/games/" (slug) } { "public page" } }
                }
            }
        }
        @if !keys.is_empty() {
            form.stack method="post" action="/dashboard/games" {
                h3 { "Create a game" }
                input type="hidden" name="csrf" value=(csrf);
                label { "Slug" input type="text" name="slug" required pattern="[a-z0-9][a-z0-9-]{1,63}" placeholder="cool-game"; }
                label { "Title" input type="text" name="title" required maxlength="200"; }
                label { "Signing key"
                    select name="pubkey" {
                        @for (pubkey, label) in &keys { option value=(pubkey) { (label) " " (pubkey) } }
                    }
                }
                button type="submit" { "Create game" }
            }
        }

        h2 { "Signing keys" }
        p.muted { "Public keys only. Private keys never leave your machine; generate one with " code { "gamedist init" } "." }
        ul { @for (pubkey, label) in &keys { li { code { (pubkey) } " " (label) } } }
        form.row method="post" action="/dashboard/keys" {
            input type="hidden" name="csrf" value=(csrf);
            input type="text" name="pubkey" required placeholder="ed25519:…" size="52";
            input type="text" name="label" placeholder="label" maxlength="100";
            button type="submit" { "Add key" }
        }

        h2 { "API tokens" }
        table {
            @for (id, name, created, last_used) in &tokens {
                tr {
                    td { (name) }
                    td { "created " (date(*created)) }
                    td { @if let Some(at) = last_used { "last used " (date(*at)) } @else { "never used" } }
                    td {
                        form.inline method="post" action="/dashboard/tokens/revoke" {
                            input type="hidden" name="csrf" value=(csrf);
                            input type="hidden" name="id" value=(id);
                            button.danger type="submit" { "Revoke" }
                        }
                    }
                }
            }
        }
        form.row method="post" action="/dashboard/tokens" {
            input type="hidden" name="csrf" value=(csrf);
            input type="text" name="name" required placeholder="token name, e.g. ci" maxlength="100";
            button type="submit" { "Create token" }
        }
    })
}

async fn dashboard(State(st): State<St>, session: Session, Query(msg): Query<Msg>) -> PortalResult {
    let user = require_user(&session)?;
    let body = dashboard_body(&st, &session, user, &msg, None).await?;
    Ok(page(&st, &session, "Dashboard", body))
}

#[derive(Debug, Deserialize)]
struct AddKeyForm {
    csrf: String,
    pubkey: String,
    #[serde(default)]
    label: String,
}

async fn dashboard_add_key(
    State(st): State<St>,
    session: Session,
    Form(form): Form<AddKeyForm>,
) -> PortalResult {
    let user = require_user(&session)?;
    session.check_csrf(&form.csrf)?;
    let key = form.pubkey.trim().parse().map_err(ApiError::from)?;
    games::register_key(&st, user, key, form.label.trim()).await?;
    Ok(back("/dashboard", "Key added"))
}

#[derive(Debug, Deserialize)]
struct TokenForm {
    csrf: String,
    name: String,
}

async fn dashboard_create_token(
    State(st): State<St>,
    session: Session,
    Form(form): Form<TokenForm>,
) -> PortalResult {
    let user = require_user(&session)?;
    session.check_csrf(&form.csrf)?;
    games::validate_text("token name", &form.name, 100, true)?;
    let token = auth::create_token(&st.db, user.id, form.name.trim(), "api").await?;
    db::audit(
        &st.db,
        user.actor(),
        "token.create",
        form.name.trim(),
        "portal",
    )
    .await;
    let body = dashboard_body(&st, &session, user, &Msg::default(), Some(&token)).await?;
    let mut response = page(&st, &session, "Dashboard", body);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct RevokeForm {
    csrf: String,
    id: i64,
}

async fn dashboard_revoke_token(
    State(st): State<St>,
    session: Session,
    Form(form): Form<RevokeForm>,
) -> PortalResult {
    let user = require_user(&session)?;
    session.check_csrf(&form.csrf)?;
    sqlx::query("DELETE FROM tokens WHERE id = ? AND user_id = ? AND kind = 'api'")
        .bind(form.id)
        .bind(user.id)
        .execute(&st.db)
        .await?;
    db::audit(
        &st.db,
        user.actor(),
        "token.revoke",
        &form.id.to_string(),
        "",
    )
    .await;
    Ok(back("/dashboard", "Token revoked"))
}

#[derive(Debug, Deserialize)]
struct CreateGameForm {
    csrf: String,
    slug: String,
    title: String,
    pubkey: String,
}

async fn dashboard_create_game(
    State(st): State<St>,
    session: Session,
    Form(form): Form<CreateGameForm>,
) -> PortalResult {
    let user = require_user(&session)?;
    session.check_csrf(&form.csrf)?;
    let key = form.pubkey.trim().parse().map_err(ApiError::from)?;
    let game = games::create_game(&st, user, form.slug.trim(), key, &form.title, "", "").await?;
    Ok(Redirect::to(&format!("/dashboard/games/{}", game.slug)).into_response())
}

async fn dashboard_game(
    State(st): State<St>,
    session: Session,
    Path(slug): Path<String>,
    Query(msg): Query<Msg>,
) -> PortalResult {
    let user = require_user(&session)?;
    let game: GameRow = games::owned_game(&st, user, &slug).await?;
    let list = games::release_list(&st, &game).await?;
    let csrf = &session.csrf;
    let body = html! {
        p { a href="/dashboard" { "← Dashboard" } }
        h1 { (game.title) }
        (flash(&msg))
        @if game.taken_down { p.error { "This game has been taken down: " (game.takedown_reason) } }
        p { "Public page: " a href={ "/games/" (slug) } { (st.game_url(&slug)) } }
        p { "Game ID " code { (game.game_id) } " · key " code { (game.developer_pubkey) } }

        form.stack method="post" action={ "/dashboard/games/" (slug) } {
            h2 { "Store page" }
            input type="hidden" name="csrf" value=(csrf);
            label { "Title" input type="text" name="title" value=(game.title) required maxlength="200"; }
            label { "Summary" input type="text" name="summary" value=(game.summary) maxlength="300"; }
            label { "Description (Markdown)" textarea name="description_md" rows="10" { (game.description_md) } }
            button type="submit" { "Save" }
            p.muted { "Icon, banner and screenshots are uploaded with " code { "gamedist assets set" } "." }
        }

        h2 { "Channels" }
        ul { @for (channel, version) in &list.channels { li { strong { (channel) } " → " (version) } } }

        h2 { "Releases" }
        @if list.releases.is_empty() { p.muted { "Nothing published yet. Use " code { "gamedist push" } "." } }
        table {
            @for release in &list.releases {
                tr {
                    td { strong { (release.version) } }
                    td { (release.created) }
                    td { (release.platforms.join(", ")) }
                    td {
                        form.row method="post" action={ "/dashboard/games/" (slug) "/channel" } {
                            input type="hidden" name="csrf" value=(csrf);
                            input type="hidden" name="version" value=(release.version);
                            input type="text" name="channel" value="stable" size="8" required;
                            button type="submit" { "Promote" }
                        }
                    }
                    td {
                        form.inline method="post" action={ "/dashboard/games/" (slug) "/unpublish" } {
                            input type="hidden" name="csrf" value=(csrf);
                            input type="hidden" name="version" value=(release.version);
                            button.danger type="submit" { "Unpublish" }
                        }
                    }
                }
            }
        }
    };
    Ok(page(&st, &session, &game.title, body))
}

#[derive(Debug, Deserialize)]
struct UpdateGameForm {
    csrf: String,
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    description_md: String,
}

async fn dashboard_update_game(
    State(st): State<St>,
    session: Session,
    Path(slug): Path<String>,
    Form(form): Form<UpdateGameForm>,
) -> PortalResult {
    let user = require_user(&session)?;
    session.check_csrf(&form.csrf)?;
    games::update_game(
        &st,
        user,
        &slug,
        Some(&form.title),
        Some(&form.summary),
        Some(&form.description_md),
        None,
    )
    .await?;
    Ok(back(&format!("/dashboard/games/{slug}"), "Saved"))
}

#[derive(Debug, Deserialize)]
struct ChannelForm {
    csrf: String,
    version: String,
    #[serde(default)]
    channel: String,
}

async fn dashboard_set_channel(
    State(st): State<St>,
    session: Session,
    Path(slug): Path<String>,
    Form(form): Form<ChannelForm>,
) -> PortalResult {
    let user = require_user(&session)?;
    session.check_csrf(&form.csrf)?;
    games::set_channel(&st, user, &slug, form.channel.trim(), &form.version).await?;
    Ok(back(
        &format!("/dashboard/games/{slug}"),
        &format!("{} now points at {}", form.channel.trim(), form.version),
    ))
}

async fn dashboard_unpublish(
    State(st): State<St>,
    session: Session,
    Path(slug): Path<String>,
    Form(form): Form<ChannelForm>,
) -> PortalResult {
    let user = require_user(&session)?;
    session.check_csrf(&form.csrf)?;
    games::delete_release(&st, user, &slug, &form.version).await?;
    Ok(back(
        &format!("/dashboard/games/{slug}"),
        &format!("Unpublished {}", form.version),
    ))
}

// ---- admin -----------------------------------------------------------------

async fn admin(State(st): State<St>, session: Session, Query(msg): Query<Msg>) -> PortalResult {
    require_admin(&session)?;
    let csrf = &session.csrf;
    let users = sqlx::query(
        "SELECT u.id, u.email, u.is_admin, u.disabled, u.quota_bytes,
                (SELECT COALESCE(SUM(size), 0) FROM chunks c WHERE c.uploader_id = u.id) AS used
         FROM users u ORDER BY u.id",
    )
    .fetch_all(&st.db)
    .await?;
    let game_rows = sqlx::query(
        "SELECT g.slug, g.title, g.taken_down, g.takedown_reason, u.email
         FROM games g JOIN users u ON u.id = g.owner_id ORDER BY g.slug",
    )
    .fetch_all(&st.db)
    .await?;
    let audit: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT at, actor, action, target, detail FROM audit_log ORDER BY id DESC LIMIT 100",
    )
    .fetch_all(&st.db)
    .await?;
    let (chunk_count, chunk_bytes): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM chunks")
            .fetch_one(&st.db)
            .await?;
    let mode = db::registration_mode(&st.db, st.cfg.registration).await?;
    let default_quota = db::default_quota(&st.db, st.cfg.default_quota_bytes).await?;

    let body = html! {
        h1 { "Administration" }
        (flash(&msg))
        p { "Storage: " strong { (human_bytes(chunk_bytes as u64)) } " in " (chunk_count) " chunks." }

        h2 { "Settings" }
        form.stack method="post" action="/admin/settings" {
            input type="hidden" name="csrf" value=(csrf);
            label { "Registration"
                select name="registration" {
                    @for m in [Registration::Open, Registration::Invite, Registration::Closed] {
                        option value=(m.as_str()) selected[m == mode] { (m.as_str()) }
                    }
                }
            }
            label { "Default quota per account, in MiB (0 = unlimited)"
                input type="number" name="default_quota_mib" min="0" value=(default_quota / (1024 * 1024)); }
            button type="submit" { "Save settings" }
        }
        form.row method="post" action="/admin/invites" {
            input type="hidden" name="csrf" value=(csrf);
            button type="submit" { "Create invite code" }
        }

        h2 { "Users" }
        table {
            thead { tr { th { "Email" } th { "Role" } th { "Storage" } th { "Quota (MiB)" } th {} } }
            @for row in &users {
                @let id: i64 = row.get("id");
                @let is_admin = row.get::<i64, _>("is_admin") != 0;
                @let disabled = row.get::<i64, _>("disabled") != 0;
                @let quota: Option<i64> = row.get("quota_bytes");
                tr {
                    td { (row.get::<String, _>("email")) @if disabled { " " span.error { "(disabled)" } } }
                    td { @if is_admin { "admin" } @else { "developer" } }
                    td { (human_bytes(row.get::<i64, _>("used") as u64)) }
                    td colspan="2" {
                        form.row method="post" action="/admin/users" {
                            input type="hidden" name="csrf" value=(csrf);
                            input type="hidden" name="id" value=(id);
                            input type="number" name="quota_mib" min="0" placeholder="default"
                                value=[quota.map(|q| q / (1024 * 1024))] size="8";
                            select name="action" {
                                option value="quota" { "Set quota" }
                                @if disabled { option value="enable" { "Enable account" } }
                                @else { option value="disable" { "Disable account" } }
                                @if is_admin { option value="demote" { "Remove admin" } }
                                @else { option value="promote" { "Make admin" } }
                            }
                            button type="submit" { "Apply" }
                        }
                    }
                }
            }
        }

        h2 { "Games and takedowns" }
        table {
            thead { tr { th { "Game" } th { "Owner" } th { "Status" } th {} } }
            @for row in &game_rows {
                @let slug: String = row.get("slug");
                @let down = row.get::<i64, _>("taken_down") != 0;
                tr {
                    td { a href={ "/games/" (slug) } { (row.get::<String, _>("title")) } " " code { (slug) } }
                    td { (row.get::<String, _>("email")) }
                    td { @if down { span.error { "taken down: " (row.get::<String, _>("takedown_reason")) } } @else { "live" } }
                    td {
                        form.row method="post" action="/admin/takedown" {
                            input type="hidden" name="csrf" value=(csrf);
                            input type="hidden" name="slug" value=(slug);
                            @if down {
                                input type="hidden" name="restore" value="1";
                                button type="submit" { "Restore" }
                            } @else {
                                input type="text" name="reason" required placeholder="reason" maxlength="2000";
                                button.danger type="submit" { "Take down" }
                            }
                        }
                    }
                }
            }
        }

        h2 { "Audit log" }
        table.audit {
            @for (at, actor, action, target, detail) in &audit {
                tr { td { (date(*at)) } td { (actor) } td { code { (action) } } td { (target) } td { (detail) } }
            }
        }
    };
    Ok(page(&st, &session, "Admin", body))
}

#[derive(Debug, Deserialize)]
struct AdminUserForm {
    csrf: String,
    id: i64,
    action: String,
    #[serde(default)]
    quota_mib: String,
}

async fn admin_update_user(
    State(st): State<St>,
    session: Session,
    Form(form): Form<AdminUserForm>,
) -> PortalResult {
    let admin = require_admin(&session)?;
    session.check_csrf(&form.csrf)?;
    let mut req = api::UpdateUser {
        id: form.id,
        disabled: None,
        is_admin: None,
        quota_bytes: None,
    };
    match form.action.as_str() {
        "disable" => req.disabled = Some(true),
        "enable" => req.disabled = Some(false),
        "promote" => req.is_admin = Some(true),
        "demote" => req.is_admin = Some(false),
        "quota" => {
            req.quota_bytes = Some(match form.quota_mib.trim() {
                "" => -1,
                value => value
                    .parse::<i64>()
                    .ok()
                    .filter(|v| *v >= 0)
                    .ok_or_else(|| ApiError::bad_request("quota must be a number of MiB"))?
                    .saturating_mul(1024 * 1024),
            })
        }
        _ => return Err(ApiError::bad_request("unknown action").into()),
    }
    api::update_user(&st, admin, &req).await?;
    Ok(back("/admin", "User updated"))
}

#[derive(Debug, Deserialize)]
struct TakedownForm {
    csrf: String,
    slug: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    restore: Option<String>,
}

async fn admin_takedown(
    State(st): State<St>,
    session: Session,
    Form(form): Form<TakedownForm>,
) -> PortalResult {
    let admin = require_admin(&session)?;
    session.check_csrf(&form.csrf)?;
    let restore = form.restore.is_some();
    games::takedown(&st, admin, &form.slug, form.reason.trim(), restore).await?;
    Ok(back(
        "/admin",
        if restore {
            "Game restored"
        } else {
            "Game taken down"
        },
    ))
}

#[derive(Debug, Deserialize)]
struct SettingsForm {
    csrf: String,
    registration: String,
    default_quota_mib: u64,
}

async fn admin_settings(
    State(st): State<St>,
    session: Session,
    Form(form): Form<SettingsForm>,
) -> PortalResult {
    let admin = require_admin(&session)?;
    session.check_csrf(&form.csrf)?;
    api::update_settings(
        &st,
        admin,
        &api::SettingsRequest {
            registration: Some(form.registration),
            default_quota_bytes: Some(form.default_quota_mib.saturating_mul(1024 * 1024)),
        },
    )
    .await?;
    Ok(back("/admin", "Settings saved"))
}

async fn admin_invite(
    State(st): State<St>,
    session: Session,
    Form(form): Form<CsrfOnly>,
) -> PortalResult {
    let admin = require_admin(&session)?;
    session.check_csrf(&form.csrf)?;
    let code = api::create_invite(&st, admin).await?;
    Ok(back("/admin", &format!("Invite code: {code}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_sanitised() {
        let html = render_markdown(
            "# Hi\n<script>alert(1)</script>\n[x](javascript:alert(1)) [ok](https://example.com) ![i](http://evil/x.png)",
        )
        .into_string();
        assert!(html.contains("<h1>Hi</h1>"));
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("javascript:"), "{html}");
        assert!(html.contains("href=\"https://example.com\""));
        assert!(!html.contains("evil"), "{html}");
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }
}
