# GameDist — Open, Decentralized Game Launcher + Distributor

> Working name: **GameDist** (rename freely; keep the scheme/URI names consistent after renaming).
> License: **MIT** for every component. No GPL/AGPL dependencies. Apache-2.0 / BSD / MIT / ISC deps are fine.

## 0. Goal

Build an open-source, decentralized alternative to Steam's distribution layer:

- **Anyone can run a distributor server** (single binary, Docker, or S3/CDN-backed).
- **Developers** register a game on any server and push builds/assets/updates with a CLI.
- **Players** paste a game URL (`https://host/games/<slug>`) into a Steam-like launcher, install, launch, and receive delta updates.
- Downloads work via **direct HTTP** or **BitTorrent** (HTTP servers double as web seeds).
- **Trust lives with the developer's signing key, not with the server.** A server can host a game but cannot tamper with it.
- No DRM, no central registry. Discovery is opt-in via "catalogs".

Non-goals for v1: payments, cloud saves, friends/chat, achievements, mod workshop. Design so they can be added as optional server modules later.

---

## 1. Architecture

```
┌──────────────┐  gamedist push   ┌──────────────────┐   HTTP + torrent   ┌──────────────┐
│  Developer   │ ───────────────► │ Distributor      │ ◄───────────────── │  Launcher    │
│  (CLI, CI)   │                  │ Server (any host)│   manifest/chunks  │  (Player)    │
└──────────────┘                  │  + tracker       │                    └──────────────┘
                                  │  + web portal    │                           ▲
                                  └──────────────────┘                           │ peers
                                           ▲                               ┌─────┴──────┐
                                   mirror / federation                     │ Other       │
                                           ▼                               │ launchers   │
                                  ┌──────────────────┐                     └────────────┘
                                  │ Other servers    │
                                  └──────────────────┘
```

Shared logic (chunking, hashing, manifests, signing, download engine) lives in one Rust crate `gamedist-core`, used by server, CLI, and launcher.

### Components

| Dir | Component | Stack |
|---|---|---|
| `protocol/` | Spec (Markdown) + JSON Schemas, versioned | — |
| `crates/core/` | Chunking, BLAKE3 hashing, Ed25519 signing, manifest types, HTTP/torrent download engine, content store, update diffing | Rust |
| `crates/server/` | Distributor server: REST API, chunk storage, torrent generation + tracker, dev accounts, admin, web portal (SSR) | Rust: `axum`, `sqlx` (SQLite default, Postgres optional), `tokio`, `askama` or `maud` templates |
| `crates/cli/` | `gamedist` developer CLI | Rust: `clap` |
| `apps/launcher/` | Player launcher, Steam-like UI | Tauri 2 (Rust backend uses `core`) + Svelte (or React) + TypeScript |
| `apps/website/` | Optional standalone dev portal / public aggregator (if not using server SSR) | SvelteKit (defer; server SSR portal is enough for v1) |
| `deploy/` | Dockerfile, docker-compose, systemd unit, Caddy/nginx examples, `install.sh` | — |
| `docs/` | mdBook or Docusaurus: Run a server, Publish a game, Protocol spec, Launcher guide | — |
| `examples/sample-game/` | Tiny cross-platform game (e.g. a Rust/SDL or Godot export) used in integration tests | — |

Key crates (all MIT/Apache/BSD): `blake3`, `ed25519-dalek`, `rand`, `serde`, `serde_json`, `jsonschema`, `reqwest`, `tokio`, `axum`, `sqlx`, `librqbit` (torrent, MIT), `tauri`, `clap`, `indicatif`, `walkdir`, `zstd` (optional chunk compression).

---

## 2. Protocol v0 (authoritative spec goes in `protocol/SPEC.md`)

### 2.1 Identifiers
- **Game URL**: `https://<host>/games/<slug>` — slug: `[a-z0-9][a-z0-9-]{1,63}`.
- **URI scheme** for one-click install: `gamedist://<host>/games/<slug>` (launcher registers as handler on all OSes).
- **Game ID** (stable across servers): `base32(sha256(developer_pubkey || slug))[0..26]`. Mirrors of the same game on different hosts share this ID.
- **Chunk ID**: lowercase hex BLAKE3 of the raw chunk bytes.

### 2.2 Chunking
- Content-defined chunking (FastCDC) with min 512 KiB / avg 2 MiB / max 8 MiB. Rationale: delta updates survive inserted bytes.
- Chunks stored and served **uncompressed** by default (games are mostly already-compressed assets). Optional `zstd` per-chunk compression flagged in manifest (`"encoding": "zstd"`); hash is always of the *decoded* bytes.

### 2.3 Keys and signing
- Developer keypair: Ed25519. Public key encoded as `ed25519:<base64url>`.
- Signed objects use a canonical JSON serialization (RFC 8785 / JCS). Signature field is `"sig": "<base64url>"` over the canonical JSON of the object **without** the `sig` field.
- Launcher pins the developer pubkey per Game ID on first install (TOFU). Key change ⇒ hard warning, update refused until user confirms.
- Servers never hold developer private keys. Server auth (API tokens) is separate from release signing.

### 2.4 Objects

**Server info** — `GET /.well-known/gamedist.json`
```json
{
  "protocol": "gamedist/0",
  "name": "Example Distributor",
  "api": "https://host.example/api/v0",
  "chunks": "https://cdn.example/chunks/",
  "tracker": "udp://host.example:6969/announce",
  "catalog": "https://host.example/api/v0/games",
  "registration": "open|invite|closed"
}
```

**Game manifest** — `GET /games/<slug>/manifest.json` (server-generated, points to releases)
```json
{
  "protocol": "gamedist/0",
  "game_id": "…",
  "slug": "cool-game",
  "developer_pubkey": "ed25519:…",
  "title": "Cool Game",
  "summary": "One-line pitch",
  "description_md": "…",
  "assets": { "icon": "<chunk>", "banner": "<chunk>", "screenshots": ["<chunk>", "…"] },
  "channels": { "stable": "1.2.0", "beta": "1.3.0-beta.1" },
  "releases": "https://host.example/games/cool-game/releases/"
}
```

**Release manifest** — `GET /games/<slug>/releases/<version>.json` (developer-signed, immutable)
```json
{
  "protocol": "gamedist/0",
  "game_id": "…",
  "slug": "cool-game",
  "version": "1.2.0",
  "created": "2026-11-16T07:00:00Z",
  "changelog_md": "…",
  "platforms": {
    "windows-x86_64": {
      "launch": { "exe": "CoolGame.exe", "args": [], "cwd": "." },
      "files": [
        { "path": "CoolGame.exe", "size": 12345678, "mode": 493,
          "chunks": [ { "id": "<blake3>", "size": 2097152 }, "…" ] }
      ],
      "total_size": 1234567890
    },
    "linux-x86_64": { "…": "…" }
  },
  "developer_pubkey": "ed25519:…",
  "sig": "…"
}
```
Paths are relative, forward-slash, no `..`, no absolute paths, NFC-normalized. `mode` is POSIX bits (executable flag matters on Linux/macOS).

**Torrent** — `GET /games/<slug>/releases/<version>-<platform>.torrent`
Server-generated v1 (+ optional hybrid v2) torrent of the **assembled install tree**, with `url-list` web seed(s) pointing at an HTTP file layout (`GET /games/<slug>/releases/<version>/<platform>/files/<path>`), so plain HTTP mirrors seed automatically. Info hash recorded in release metadata by the server (not in the signed manifest).

### 2.5 REST API (`/api/v0`)

Public:
```
GET  /games?page=&q=                          catalog (if server exposes one)
GET  /games/{slug}                            game manifest (same as /games/{slug}/manifest.json)
GET  /games/{slug}/releases                   list versions + channels + info hashes
GET  /games/{slug}/releases/{ver}             signed release manifest
GET  /chunks/{id}                             immutable; Cache-Control: public, max-age=31536000, immutable
HEAD /chunks/{id}
GET  /games/{slug}/releases/{ver}/{plat}/files/{path}   web-seed file layout
```
Developer (Bearer token):
```
POST /auth/register   {email, password}             (if registration open/invite)
POST /auth/token      {email, password} -> token
POST /dev/keys        register developer pubkey to account
POST /games           {slug, developer_pubkey, title, …}
PATCH /games/{slug}   metadata/assets
POST /chunks/missing  {ids:[…]} -> {missing:[…]}      batch existence check
PUT  /chunks/{id}     raw body; server verifies BLAKE3, 201/204
POST /games/{slug}/releases      signed release manifest; server verifies sig, all chunks present
POST /games/{slug}/channels      {channel, version}  promote
DELETE /games/{slug}/releases/{ver}  (unpublish, chunks GC'd later)
```
Admin:
```
GET/POST /admin/users, /admin/games, /admin/takedowns, /admin/settings
```
Errors: RFC 7807 problem+json. Rate limiting on auth and uploads.

---

## 3. Repository layout

```
gamedist/
├── LICENSE                      MIT
├── README.md
├── CONTRIBUTING.md              DCO sign-off, no CLA
├── Cargo.toml                   workspace
├── protocol/
│   ├── SPEC.md
│   └── schemas/*.schema.json
├── crates/
│   ├── core/
│   ├── server/
│   └── cli/
├── apps/
│   └── launcher/                Tauri + Svelte
├── examples/sample-game/
├── deploy/
│   ├── Dockerfile.server
│   ├── docker-compose.yml
│   ├── install.sh
│   └── gamedist-server.service
├── docs/
└── .github/workflows/           ci.yml, release.yml
```

---

## 4. Developer CLI (`gamedist`)

```
gamedist init                       create gamedist.toml, generate keypair (~/.config/gamedist/keys/<name>.key)
gamedist key export/import
gamedist login <server-url>         store token in OS keyring
gamedist game create <slug> --title … --server <url>
gamedist push <build-dir> --platform windows-x86_64 --version 1.2.0 [--channel beta] [--changelog file.md]
gamedist promote <version> --channel stable
gamedist assets set --icon icon.png --banner banner.png --screenshots a.png b.png
gamedist mirror <other-server-url> [--version …]   re-publish signed releases to another server
gamedist status / releases list
gamedist verify <release.json>      offline signature + schema check
```
`push` algorithm: walk dir → chunk+hash in parallel → `POST /chunks/missing` → upload missing chunks (parallel, resumable, retry) → build + sign manifest → `POST releases`. Idempotent re-runs upload nothing.

`gamedist.toml` example:
```toml
slug = "cool-game"
title = "Cool Game"
key = "cool-game"            # key name
servers = ["https://host.example"]

[platforms.windows-x86_64]
launch = { exe = "CoolGame.exe" }
[platforms.linux-x86_64]
launch = { exe = "CoolGame.x86_64" }
```

---

## 5. Launcher (player app)

Steam-like layout: left sidebar (Library list + search), top nav (Library / Store-ish "Discover" / Downloads / Settings), main pane (game page with banner, play button, version, size, changelog; library grid view).

Core flows:
1. **Add game**: paste URL or `gamedist://` link → fetch server info + game manifest → show game page → choose platform (auto) and install dir.
2. **Install**: select transport (direct / torrent+direct / torrent only) → download chunks into content store (`<data>/store/<id[0..2]>/<id>`) → assemble files → verify → write `installed.json` (game_id, version, platform, pinned pubkey, install path).
3. **Launch**: run `launch.exe` with cwd; on Linux, if only a Windows build exists, optional Proton/Wine runner (configurable path; detect Steam's Proton/Heroic's Wine-GE).
4. **Update check**: on startup and on game page open; compare channel version; verify signature + pinned key; show delta size (chunks not in local store); apply in-place with atomic file swaps and rollback on failure.
5. **Verify files**: rehash installed tree against manifest, re-download bad chunks.
6. **Seeding**: optional, per game, with upload limit; shows ratio.
7. **Catalogs**: Settings → add server URL → its `/games` appears in Discover.
8. **Headless mode**: `gamedist-launcher --headless install <url> --dir …` for CI/integration tests.

Download engine (in `core`): N parallel HTTP range/chunk requests, resumable, per-chunk verification, bandwidth limit, mixed with `librqbit` torrent session that writes to the same assembled tree; HTTP path used to fill missing pieces (web seed). Progress events via channel → Tauri events → UI.

---

## 6. Server

- Single static binary. `gamedist-server --config server.toml` or env vars.
- Storage backends: `local` (dir) and `s3` (any S3-compatible). Chunks are immutable → trivially CDN-cacheable.
- DB: SQLite default (`sqlx` migrations), Postgres optional.
- Built-in UDP+HTTP tracker (`librqbit` tracker or small custom one) for its own torrents; configurable external trackers.
- Web portal (SSR, same binary): landing, game pages (public), dev dashboard (games, releases, channels, tokens, keys, stats), admin (users, takedowns, registration mode, storage usage).
- Abuse tooling from day one: registration modes (`open|invite|closed`), per-account storage quotas, takedown endpoint that unpublishes + records reason, audit log.
- Metrics endpoint (`/metrics`, Prometheus).
- Background jobs: torrent generation after release publish, chunk GC (unreferenced for >7 days), stats rollup.

---

## 7. Phased plan with acceptance criteria

Work top to bottom. Each phase must have passing CI before the next starts.

### Phase 0 — Spec & skeleton
- [ ] Workspace scaffold, MIT license, CI (fmt, clippy, test on Linux/Windows/macOS).
- [ ] `protocol/SPEC.md` written from Section 2; JSON Schemas for server-info, game manifest, release manifest.
- [ ] `core`: types + serde + schema validation + JCS canonicalization + Ed25519 sign/verify + FastCDC chunker + BLAKE3.
- **Done when**: `cargo test` round-trips sign→verify, chunker is deterministic, schema tests pass against fixture files.

### Phase 1 — Server MVP + CLI push (HTTP only)
- [ ] Server: auth, games, chunks (local storage), releases, channels, catalog, well-known, static chunk serving with correct cache headers.
- [ ] CLI: `init`, `login`, `game create`, `push`, `promote`, `verify`.
- [ ] `examples/sample-game` with Windows + Linux builds produced in CI.
- **Done when**: integration test spins up server in a temp dir, pushes sample game, re-push uploads 0 chunks, `curl` fetches manifest and all chunks verify.

### Phase 2 — Launcher MVP
- [ ] Tauri app: add-by-URL, game page, install (direct HTTP), launch, update check + delta update, verify files, library grid, settings (install dir, bandwidth).
- [ ] `gamedist://` URI handler registration on Win/Linux/macOS.
- [ ] Headless CLI mode.
- **Done when**: integration test: push v1 → headless install → push v2 with one changed file → headless update downloads only changed chunks → launch succeeds (sample game exits 0 with `--selftest`).

### Phase 3 — Torrent transport
- [ ] Server generates torrents with web seeds + runs tracker.
- [ ] `core` download engine mixes torrent + HTTP; launcher transport toggle; seeding toggle + limits.
- **Done when**: test with two headless launchers: second one gets ≥50% of pieces from the first (HTTP rate-limited to prove it).

### Phase 4 — Web portal & abuse tooling
- [ ] SSR portal: public game pages, dev dashboard, API tokens, key management, admin pages, takedowns, quotas, audit log.
- [ ] `deploy/`: Dockerfile, compose with Caddy TLS, `install.sh`, systemd unit. Docs: "Run a server in 5 minutes".
- **Done when**: fresh VPS → `install.sh` → publish game from CLI → install from launcher, in under 10 minutes following docs only.

### Phase 5 — Federation
- [ ] `gamedist mirror`; launcher multi-catalog Discover with search; server `catalog` export/import; optional public aggregator spec (`/api/v0/export.jsonl`).
- [ ] Key-change UX and documented key-rotation procedure (signed rotation statement `old_key → new_key`).
- **Done when**: same game installed from two different hosts shares Game ID, pinned key, and content store.

### Phase 6 — Polish & optional modules
- Proton/Wine runner on Linux; code signing for launcher binaries; auto-update of launcher itself (via its own protocol); news feed on game pages; module interface for future payments/cloud saves.

---

## 8. Engineering rules for the agent

- Rust 2021, `cargo fmt` + `clippy -D warnings`. Deny `unsafe` outside `core` hashing hot paths.
- Every public API in `core` has unit tests; every server endpoint has an integration test (`axum::test` + temp SQLite).
- Never trust manifest paths: reject `..`, absolute, drive letters, symlinks on assembly.
- All network downloads verify BLAKE3 before writing to final location; write to temp then atomic rename.
- Secrets (tokens, private keys) never logged; use OS keyring (`keyring` crate) in CLI/launcher.
- Keep `core` free of UI/server deps so it compiles for Tauri, server, CLI alike.
- Each phase ends with updated `docs/` and a `CHANGELOG.md` entry.
- Check every new dependency's license (`cargo deny` with MIT/Apache-2.0/BSD/ISC/Zlib/Unicode allowlist; deny GPL/AGPL/LGPL).

---

## 9. Open questions (decide in Phase 0, record in `docs/decisions/`)

1. Final project name + URI scheme.
2. Torrent v1-only vs hybrid v2 (hybrid preferred if `librqbit` supports creation).
3. Tauri frontend framework: Svelte (default) vs React.
4. Whether the standalone `apps/website` is needed or server SSR portal suffices (default: SSR only for v1).
5. Launcher auto-update channel: self-hosted via the protocol vs GitHub Releases (default: both, protocol primary).
