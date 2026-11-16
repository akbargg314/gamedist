# Changelog

All notable changes to this project are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

- Added the project website in `site/` (landing page, downloads from GitHub Releases, the mdBook under `docs/`), deployed to GitHub Pages by `.github/workflows/pages.yml`. See ADR 0010, which supersedes ADR 0004.

### Phase 0 — Spec and skeleton

- Added Cargo workspace (`gamedist-core`, `gamedist-server`, `gamedist-cli`), MIT license, `cargo-deny` license allowlist.
- Added `protocol/SPEC.md` (protocol `gamedist/0`) and JSON Schemas for server info, game manifest, release manifest and key-rotation statements, with valid and invalid fixtures.
- Added `gamedist-core`: identifiers (slug, Game ID, chunk ID, version, platform, game URLs including `gamedist://` and `gamedist+http://`), RFC 8785 canonical JSON restricted to integers, Ed25519 signing and strict verification, manifest types, schema validation, FastCDC chunker with BLAKE3 IDs, manifest path validation.
- Added CI workflow (fmt, clippy, tests on Linux/Windows/macOS, cargo-deny).

### Phase 1 — Server MVP and CLI push

- Added `gamedist-server` with `serve`, `user-add` and `gc` subcommands, TOML config with `GAMEDIST_*` environment overrides.
- Added SQLite database and local-directory chunk store.
- Added public endpoints: `/.well-known/gamedist.json`, catalog with search and paging, game manifests, release lists, signed release manifests, immutable chunk serving with long-lived cache headers, RFC 7807 errors.
- Added developer endpoints: register, token, `/dev/me`, keys, game create/patch, batch missing-chunk check, chunk upload with BLAKE3 verification, release publish with full signature and chunk verification, channel promotion, release unpublish.
- Added per-IP rate limits on authentication and uploads.
- Added `gamedist` CLI: `init`, `key show/export/import`, `login` (with `--register`), `game create`, `push` (parallel, retrying, idempotent; multi-platform via `--build`), `promote`, `assets set`, `status`, `releases list`, `verify`.
- Added API token storage in the OS keyring with a 0600 file fallback; `GAMEDIST_TOKEN`, `GAMEDIST_PASSWORD`, `GAMEDIST_TOKEN_STORE=file` for CI.

### Phase 2 — Launcher MVP

- Added content-addressed chunk store, HTTP download engine (parallel, verified, rate-limited), install planning with delta downloads and chunk harvesting from the installed version, atomic apply with journaled rollback, interrupted-install recovery and tree verification in `gamedist-core`.
- Added the launcher engine `gamedist-launcher` (`crates/launcher`): library with trust-on-first-use key pinning, add by URL (`https://`, `gamedist://`, `gamedist+http://`), install, delta update, verify/repair, launch (Wine-compatible runner for Windows builds on other systems: configured path or `wine` on `PATH`), uninstall, downgrade refusal unless allowed, settings, `installed.json` markers. The chunk store is pruned after install unless `keep_chunks` is on.
- Added the headless CLI: `gamedist-launcher --headless add | install | update | check | verify | launch | list | uninstall | catalog | discover | settings | seed`, with `--data-dir`, `--json`, `--quiet`.
- Added the Tauri 2 + Svelte 5 GUI in `apps/launcher` (library, game page, Discover, Downloads, Settings, key-change warning, `gamedist://` handler). The frontend builds and type-checks; the Rust shell compiles and has been run on Linux only.
- Added progress detail in the launcher engine: download progress names its source (server or peers), verify and chunk harvesting report bytes as they go, a repair reports each of its steps, and the installed size is recorded per game. Operations on the same game (install, verify, uninstall, seed) are serialised.
- Added `examples/sample-game` (`--selftest`) and the end-to-end test package `gamedist-e2e`: install, delta update that downloads only the changed chunk, `launch -- --selftest` exits 0; verify/repair; refusal of a corrupted chunk, a swapped developer key and a rolled-back channel.

### Phase 3 — Torrent transport

- Added BitTorrent v1 metainfo generation per release platform with BEP 19 web seeds and a fixed `files` torrent name.
- Added HTTP web-seed file layout with single-range `Range` support.
- Added built-in HTTP tracker at `/announce` (compact peers) and `external_trackers` config.
- Added the launcher BitTorrent client (cargo feature `torrent`, on by default, librqbit): transports `direct`, `torrent-direct` and `torrent`; torrents accepted only if they match the signed manifest exactly; downloads into a scratch directory with per-chunk BLAKE3 verification; HTTP fetches the rest after the torrent phase completes or stalls (`torrent_stall_secs`, default 20). DHT and local peer discovery are disabled. See ADR 0009.
- Added seeding of verified installs (`seed`, `upload_limit`, `listen_port` settings; `gamedist-launcher --headless seed`). The bandwidth limit applies to BitTorrent downloads too.
- Added the end-to-end test: a second launcher gets at least half of the game from the first over BitTorrent; torrent-only install; fallback to HTTP with no seeders.

### Phase 4 — Web portal and abuse tooling

- Added server-rendered portal: landing page, public game pages, login/registration, developer dashboard (games, releases, channels, API tokens, keys).
- Added admin pages and API: users (disable, admin flag, quotas), registration mode (`open`/`invite`/`closed`), invites, takedowns (HTTP 451 on public endpoints), audit log, server settings.
- Added per-account storage quotas, hourly chunk garbage collection with a grace period, Prometheus `/metrics`.
- Added `deploy/` (Dockerfile, Compose with Caddy, nginx example, systemd unit, `install.sh`) and mdBook documentation. These deployment files have not been tested end to end.
- Changed: the server shuts down gracefully on SIGTERM (Unix) as well as SIGINT; the systemd unit and the Docker image now use the default stop signal (SIGTERM).

### Phase 5 — Federation

- Added `gamedist mirror` to re-publish signed releases, assets and channels to another server.
- Added catalog export at `/api/v0/export.jsonl`.
- Added key-rotation statements: `gamedist key rotate`, `POST /api/v0/games/{slug}/rotations`, rotation chains in the game manifest and chain verification in `gamedist-core`.
- Added launcher federation: the same game added from a second host becomes an extra source of the same library entry; catalogs and Discover search across configured servers.
- Added launcher key-change handling: a different developer key is refused (`KeyChanged`) unless a valid signed rotation chain links it to the pinned key (followed automatically, install carried over) or the user explicitly accepts it (`--accept-key-change`).
- Added end-to-end tests: the same game from two hosts shares Game ID, pinned key and content store; a key rotation is followed without user interaction; catalogs, Discover, settings and the bandwidth limit.
