# Run a server in 5 minutes

> **Status:** the server itself is covered by integration and end-to-end
> tests. The deployment files in `deploy/` (`install.sh`, the Docker image,
> Compose with Caddy, the systemd unit) have **not** been tested end to end
> yet, so the "fresh VPS in minutes" path is unverified. Please report what
> breaks.

`gamedist-server` is a single binary with an embedded SQLite database and a
local chunk directory. Put it behind a TLS reverse proxy and give it a public
URL. Three ways to get there:

| Path | Best for |
|---|---|
| [`install.sh`](#path-1-installsh-on-a-fresh-vps) | A fresh Debian/Ubuntu VPS; sets up systemd + Caddy |
| [Docker Compose](#path-2-docker-compose) | Hosts that already run Docker |
| [Manual](#path-3-manual) | Anything else |

What you need in every case: a DNS name pointing at the machine, and ports 80
and 443 reachable (for the certificate and for players).

## Path 1: `install.sh` on a fresh VPS

From a checkout of the repository (or with the script and
`deploy/gamedist-server.service` copied to the machine):

```sh
sudo ./deploy/install.sh --domain games.example.org --email you@example.org
```

The script:

1. installs the binary from `--binary PATH`, else builds it with `cargo` if
   available in the checkout, else downloads a release archive
   (`RELEASE_BASE_URL` at the top of the script — no releases are published
   yet, so use one of the first two options for now);
2. creates the `gamedist` system user, `/etc/gamedist` and `/var/lib/gamedist`;
3. writes `/etc/gamedist/server.toml` with `public_url`, `trust_proxy = true`,
   `listen = "127.0.0.1:8080"` and registration `closed` (choose with
   `--registration open|invite|closed`). An existing config is never
   overwritten unless you pass `--force` (a backup is kept);
4. installs and enables `gamedist-server.service`;
5. installs Caddy (distribution package, or the official Caddy apt repository)
   and writes `/etc/caddy/Caddyfile` for your domain. A Caddyfile with local
   changes is left alone unless `--force`. Use `--skip-caddy` to bring your own
   proxy;
6. creates the admin account with `gamedist-server user-add --admin`, passing
   a generated password through the `GAMEDIST_PASSWORD` environment variable,
   and prints that password once at the end. Use `--prompt-password` to type
   your own.

Re-running the script upgrades the binary and restarts the service; it keeps
the config, the Caddyfile and the data. See `install.sh --help`.

## Path 2: Docker Compose

```sh
cd deploy
cp .env.example .env            # set GAMEDIST_DOMAIN
docker compose up -d --build
docker compose exec -e GAMEDIST_PASSWORD='a-long-password' gamedist \
  gamedist-server user-add --email you@example.org --admin
```

`docker-compose.yml` runs the server (built from `deploy/Dockerfile.server`)
with `GAMEDIST_PUBLIC_URL=https://${GAMEDIST_DOMAIN}` and
`GAMEDIST_TRUST_PROXY=true`, and Caddy in front of it with automatic HTTPS. Data
lives in the named volume `gamedist-data` (mounted at `/data`).

The image alone:

```sh
docker build -f deploy/Dockerfile.server -t gamedist-server .
docker run -d -p 127.0.0.1:8080:8080 -v gamedist-data:/data \
  -e GAMEDIST_PUBLIC_URL=https://games.example.org -e GAMEDIST_TRUST_PROXY=true \
  gamedist-server
```

## Path 3: Manual

```sh
cargo build -p gamedist-server --release
sudo install -m755 target/release/gamedist-server /usr/local/bin/
sudo useradd --system --user-group --home-dir /var/lib/gamedist --shell /usr/sbin/nologin gamedist
sudo install -d -m750 -o root -g gamedist /etc/gamedist
sudo install -m640 -o root -g gamedist deploy/server.toml.example /etc/gamedist/server.toml
# edit public_url, data_dir = "/var/lib/gamedist", trust_proxy = true
sudo install -m644 deploy/gamedist-server.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now gamedist-server
```

Then configure a reverse proxy (`deploy/Caddyfile` or
`deploy/nginx.conf.example`) and create the first admin (below).

For a quick local test, no config is needed:

```sh
gamedist-server            # listens on 127.0.0.1:8080, data in ./data
```

## Configuration reference

`gamedist-server --config server.toml` (or `GAMEDIST_CONFIG=…`). Every key is
optional; unknown keys are an error. Environment variables override the file
where listed. A commented example with all keys is in
`deploy/server.toml.example`.

| Key | Env | Default | Meaning |
|---|---|---|---|
| `name` | `GAMEDIST_NAME` | `"GameDist server"` | Name shown in the portal and server info |
| `listen` | `GAMEDIST_LISTEN` | `127.0.0.1:8080` | Listen address |
| `public_url` | `GAMEDIST_PUBLIC_URL` | empty | External base URL (`https://…`, no trailing slash). Empty means "derive from `listen`", for local testing only |
| `data_dir` | `GAMEDIST_DATA_DIR` | `data` | Database (`gamedist.sqlite`) and chunks (`chunks/`) |
| `registration` | `GAMEDIST_REGISTRATION` | `closed` | Initial registration mode: `open`, `invite`, `closed` |
| `default_quota_bytes` | `GAMEDIST_DEFAULT_QUOTA_BYTES` | `21474836480` (20 GiB) | Initial per-account quota; `0` = unlimited |
| `chunks_url` | `GAMEDIST_CHUNKS_URL` | unset | Advertise this chunk URL prefix (CDN) instead of `<public_url>/chunks/` |
| `catalog` | `GAMEDIST_CATALOG` | `true` | Expose `/api/v0/games`, `/api/v0/export.jsonl` and the landing-page list |
| `tracker` | `GAMEDIST_TRACKER` | `true` | Run the HTTP tracker at `/announce` |
| `external_trackers` | — | `[]` | Extra announce URLs added to generated torrents |
| `tracker_interval_secs` | — | `120` | Announce interval sent to clients |
| `trust_proxy` | `GAMEDIST_TRUST_PROXY` | `false` | Use the right-most `X-Forwarded-For` entry as client IP |
| `auth_rate_per_minute` | — | `20` | Login/registration attempts per IP per minute |
| `upload_rate_per_minute` | — | `6000` | Chunk uploads per IP per minute |
| `gc_grace_days` | — | `7` | Age before unreferenced chunks are deleted |

`registration` and `default_quota_bytes` are only initial values: once an
admin changes them on the `/admin` page, the stored setting wins.

Log verbosity is controlled by `RUST_LOG` (default `info`).

## Registration modes

| Mode | Who can create an account |
|---|---|
| `closed` (default) | Nobody via the web or API; the operator uses `gamedist-server user-add` |
| `invite` | Anyone with an unused invite code. Admins create codes on `/admin` (or `POST /api/v0/admin/invites`). Developers register with `gamedist login <url> --register --invite <code>` or on `/register` |
| `open` | Anyone. On an open server the very first account to register becomes administrator |

Switch modes at any time on the `/admin` page.

## The first admin

```sh
sudo -u gamedist GAMEDIST_PASSWORD='at-least-10-chars' \
  gamedist-server --config /etc/gamedist/server.toml user-add --email you@example.org --admin
```

`user-add` works in every registration mode and can run while the server is
up. Without `GAMEDIST_PASSWORD` it reads the password from standard input.
Passwords must be at least 10 characters. Run it as the service user so the
database files keep the right owner.

## Backups

Everything lives in `data_dir`: `gamedist.sqlite` (plus its `-wal`/`-shm`
files; the database runs in WAL mode) and `chunks/`. Chunks are immutable and
can be copied incrementally (e.g. `rsync`). For a consistent database copy,
either stop the service briefly or use `sqlite3 gamedist.sqlite ".backup
backup.sqlite"`. Back up the config file too.

The server holds no developer signing keys, so a lost server loses hosting,
not the ability to publish: developers can push or `gamedist mirror` their
releases to a new server.

## CDN in front of chunks

Chunks at `/chunks/<id>` are content-addressed and served with
`Cache-Control: public, max-age=31536000, immutable`, so any pull-through CDN
can cache them forever. Point the CDN origin at `https://<your host>/chunks/`
and set:

```toml
chunks_url = "https://cdn.example.org/chunks/"
```

Server info (`/.well-known/gamedist.json`) then advertises the CDN as the
chunk location. Manifests and the API are still served by the server itself.
Integrity does not depend on the CDN: clients verify every chunk.

## Reverse proxy notes

- Set `trust_proxy = true` only when the server is reachable solely through
  your proxy (listen on `127.0.0.1`), and make the proxy append the client
  address to `X-Forwarded-For` (Caddy does by default; nginx:
  `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for`). Rate limits
  and the tracker use this address.
- Allow request bodies of at least 64 MiB (release manifests) — the examples
  use 80 MB. Chunk uploads are at most 8 MiB.
- Do not buffer uploads to disk (`proxy_request_buffering off` in nginx).
- Pass `Range` requests through unchanged; the web-seed file layout uses them.
- The server shuts down gracefully on SIGINT (Ctrl-C) and, on Unix, on
  SIGTERM, which is the default stop signal of both systemd and Docker. The
  unit and the image therefore keep the default.

Examples: `deploy/Caddyfile`, `deploy/nginx.conf.example`.

## Torrents and tracker

After a release is published the server generates one BitTorrent v1 torrent
per platform (with the server's HTTP file layout as a BEP 19 web seed) at
`/games/<slug>/releases/<version>-<platform>.torrent`. With `tracker = true`
the built-in **HTTP** tracker at `/announce` serves only this server's
torrents. There is no UDP tracker. Add public trackers with
`external_trackers`.

Launchers with the `torrent-direct` or `torrent` transport fetch this torrent
from the server they are installing from, find peers through the trackers
named in it (the launcher has DHT and local peer discovery turned off), and
use the server's chunk endpoint for anything the swarm does not deliver. The
launcher's BitTorrent client does not use the web seed; the `url-list` entry
is there for other BitTorrent clients. With `tracker = false` and no
`external_trackers`, launchers cannot find each other. See
[Launcher](launcher.md#transports-and-bittorrent).

## Metrics

`GET /metrics` returns Prometheus text format: counters for chunks and bytes
served and uploaded, web-seed bytes served, releases published, tracker
announces and authentication failures, plus gauges for users, games,
releases, stored chunks and bytes, and tracker peers. The endpoint is not
authenticated; block it at the proxy if you do not want it public (both proxy
examples show how).

## Garbage collection

Unpublishing a release (`DELETE /api/v0/games/<slug>/releases/<version>`, or
the dashboard) does not delete chunks immediately. An hourly job deletes
chunks that no release or store-page asset references and that are older than
`gc_grace_days`. Run it by hand with:

```sh
sudo -u gamedist gamedist-server --config /etc/gamedist/server.toml gc        # respects the grace period
sudo -u gamedist gamedist-server --config /etc/gamedist/server.toml gc --now  # deletes every unreferenced chunk
```

## Moderation: takedowns, quotas, audit log

Sign in as an admin and open `/admin`:

- **Settings** — registration mode, default quota, create invite codes.
- **Users** — set a per-account quota, disable or re-enable accounts, grant or
  revoke admin.
- **Games and takedowns** — take a game down with a reason, or restore it. A
  taken-down game answers HTTP 451 on every public endpoint.
- **Audit log** — account, settings, takedown, key-rotation and GC events.

The same functions are available as JSON under `/api/v0/admin/…` (`users`,
`games`, `takedowns`, `settings`, `invites`, `audit`) with an admin token.

Uploads beyond an account's quota are refused.

## Not implemented yet

- S3-compatible chunk storage (only the local directory backend exists).
- PostgreSQL (only SQLite).
- UDP tracker (only HTTP `/announce`).
- Catalog import from other servers (export exists).
