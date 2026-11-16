# GameDist

GameDist is an open, decentralized game distribution system: a distributor
server anyone can run, a developer CLI for publishing builds, and a player
launcher that installs and updates games from any server. There is no central
registry, no DRM and no payment layer. Everything is MIT-licensed.

Website and documentation: <https://akbargg314.github.io/gamedist/>

- Developers register a game on any server and push builds with `gamedist push`.
- Players add a game by URL (`https://host/games/<slug>` or
  `gamedist://host/games/<slug>`), install it, and receive delta updates.
- Content is split into content-addressed chunks, served over plain HTTP; the
  server also generates BitTorrent v1 torrents with HTTP web seeds and runs an
  HTTP tracker, so launchers can download from and seed to each other.

## Trust model

1. Every release manifest is signed by the developer's Ed25519 key; the server never holds that key.
2. The server's API tokens only authorise uploads. A server cannot forge or alter a release.
3. The launcher pins the developer key per Game ID on first install (trust on first use).
4. A release signed by any other key is refused unless the user confirms, or a signed key-rotation statement links the keys.
5. Every chunk is verified against its BLAKE3 ID before it is written to its final location, whether it came over HTTP or BitTorrent. Torrents are unsigned and only accepted if they describe exactly the files of the signed manifest.

## Architecture

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

Shared logic (chunking, hashing, manifests, signing, the download and install
engine) lives in `gamedist-core`, which the server, CLI and launcher all use.

## Quickstart

### Run a server

The deployment files below have not been tested end to end yet (see
[Status](#status)). On a fresh Debian/Ubuntu VPS with a DNS name pointing at
it:

```sh
sudo ./deploy/install.sh --domain games.example.org --email you@example.org
```

Or with Docker Compose (Caddy terminates TLS):

```sh
cd deploy
cp .env.example .env        # set GAMEDIST_DOMAIN
docker compose up -d
docker compose exec -e GAMEDIST_PASSWORD='a-long-password' gamedist \
  gamedist-server user-add --email you@example.org --admin
```

See [docs/src/run-a-server.md](docs/src/run-a-server.md).

### Publish a game

```sh
gamedist init cool-game --title "Cool Game" --server https://games.example.org
gamedist login https://games.example.org
gamedist game create --summary "One-line pitch"
gamedist push build/linux --platform linux-x86_64 --exe CoolGame.x86_64 \
  --version 1.0.0 --channel stable
```

Back up your signing key (`gamedist key export cool-game`). See
[docs/src/publish-a-game.md](docs/src/publish-a-game.md).

### Play

With the headless launcher (`cargo build -p gamedist-launcher --release`):

```sh
gamedist-launcher --headless install https://games.example.org/games/cool-game --dir ~/Games/cool-game
gamedist-launcher --headless launch cool-game
gamedist-launcher --headless update cool-game
```

To download from other players and seed to them:

```sh
gamedist-launcher --headless settings --transport torrent-direct
gamedist-launcher --headless seed          # seeds installed games until Ctrl-C
```

In the desktop app (`apps/launcher`), paste the game URL or open a
`gamedist://` link. See [docs/src/launcher.md](docs/src/launcher.md) for all
commands, transports, seeding and the key-change warning.

## Repository layout

| Path | Contents |
|---|---|
| `protocol/` | `SPEC.md` (authoritative protocol v0), JSON Schemas, test fixtures |
| `crates/core/` | `gamedist-core`: IDs, JCS, Ed25519 signing, manifests, FastCDC chunker, content store, install/update assembly, torrent metainfo, HTTP client and download engine |
| `crates/server/` | `gamedist-server`: REST API, chunk storage, torrents, HTTP tracker, SSR web portal, metrics |
| `crates/cli/` | `gamedist`: developer CLI |
| `crates/launcher/` | `gamedist-launcher`: launcher engine (library, key pinning, install, delta update, verify/repair, launch, BitTorrent transport and seeding, catalogs) and headless CLI |
| `apps/launcher/` | Tauri 2 + Svelte 5 launcher GUI; `src-tauri/` is a separate Cargo workspace (see its README) |
| `examples/sample-game/` | Tiny dependency-free game (`sample-game`) used in the end-to-end tests |
| `tests/e2e/` | `gamedist-e2e`: end-to-end tests across server, CLI, launcher and sample game |
| `deploy/` | Dockerfile, Compose + Caddy, nginx example, systemd unit, `install.sh` |
| `docs/` | mdBook user documentation and `docs/decisions/` (ADRs) |

## Building and testing

Requires a stable Rust toolchain.

```sh
cargo build --workspace --release
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check licenses bans sources
```

Binaries: `target/release/gamedist-server`, `target/release/gamedist`,
`target/release/gamedist-launcher` (headless launcher). The end-to-end tests
alone: `cargo test -p gamedist-e2e`.

The GUI is built separately from `apps/launcher` (it needs the system
webview, WebKitGTK on Linux); see
[apps/launcher/README.md](apps/launcher/README.md).

Build the docs with `mdbook build docs` (output in `docs/book/`).

## Status

Status against the phases of the project plan. Three states:

- **Tested**: implemented, and covered by tests in this repository
  (`cargo test --workspace`) on Linux.
- **Unverified**: implemented (or configured), but never built, run or tested
  in the development environment.
- **Not implemented**.

Windows and macOS have never been built or tested; the CI workflow lists
them, but CI has not run yet.

| Phase | Item | Status |
|---|---|---|
| 0 | Protocol spec, JSON Schemas and fixtures | Tested |
| 0 | `gamedist-core`: JCS, Ed25519, manifests, FastCDC chunker, BLAKE3, path validation | Tested |
| 0 | CI workflow (fmt, clippy, tests on Linux/Windows/macOS, cargo-deny) | Unverified |
| 1 | Server: auth, games, chunks (local storage), releases, channels, catalog, well-known, cache headers | Tested |
| 1 | CLI: `init`, `login`, `game create`, `push`, `promote`, `verify` (re-push uploads 0 chunks; every served chunk verifies) | Tested |
| 1 | Sample game Linux and Windows builds in CI | Unverified (the e2e tests build and run the Linux binary) |
| 2 | Launcher engine and headless CLI: add by URL with key pinning, install, delta update, verify/repair, launch, downgrade refusal, settings | Tested |
| 2 | Acceptance: install, update downloads only the changed chunk, `launch -- --selftest` exits 0 | Tested |
| 2 | Wine-compatible runner for Windows builds on Linux/macOS (configured path or `wine` on `PATH`) | Unverified |
| 2 | GUI (Tauri 2 + Svelte 5) | Partly tested: the frontend builds and type-checks; the Rust shell (`src-tauri`) compiles and has been run by hand on Linux (add, install and launch of the sample game). No automated GUI tests; never built on Windows or macOS |
| 2 | `gamedist://` URI handler registration on Windows/Linux/macOS | Unverified (part of the GUI shell) |
| 3 | Server: v1 torrents with BEP 19 web seeds, HTTP tracker at `/announce` | Tested |
| 3 | Launcher transports `direct` / `torrent-direct` / `torrent`; seeding only verified installs | Tested |
| 3 | Acceptance: a second launcher gets at least 50% of the game from the first over BitTorrent; torrent-only install; fallback to HTTP with no seeders | Tested |
| 3 | Bandwidth limit on BitTorrent downloads (the HTTP limit is tested); upload limit | Unverified |
| 3 | UDP tracker, hybrid v2 torrents | Not implemented |
| 4 | SSR portal, dev dashboard, API tokens, key management, admin pages, takedowns, quotas, audit log, `/metrics` | Tested |
| 4 | `deploy/`: Dockerfile, Compose with Caddy, `install.sh`, systemd unit | Unverified (Docker has not been available; `install.sh` has not been run on a VPS) |
| 4 | Acceptance: fresh VPS to installed game in under 10 minutes | Unverified |
| 5 | `gamedist mirror`; catalog export (`/api/v0/export.jsonl`); key rotation statements | Tested |
| 5 | Launcher: mirrors as extra sources, multi-catalog Discover with search | Tested |
| 5 | Key-change refusal (`KeyChanged`) and explicit acceptance; signed rotation followed automatically | Tested (headless/engine; the GUI warning dialog is unverified) |
| 5 | Acceptance: the same game from two hosts shares Game ID, pinned key and content store | Tested |
| 5 | Server-side catalog import | Not implemented |
| 6 | Proton / Wine-GE detection, launcher self-update, code signing, news feed, module interface | Not implemented |
| — | S3 storage backend, PostgreSQL, zstd chunk encoding | Not implemented |
| — | Project website (`site/`, published with the docs by `pages.yml`) | Unverified: builds and link-checks locally; never deployed |

## License

MIT. See [LICENSE](LICENSE). Contributions are accepted under the same license
with a DCO sign-off; see [CONTRIBUTING.md](CONTRIBUTING.md).
