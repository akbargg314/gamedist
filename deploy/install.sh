#!/usr/bin/env bash
# GameDist server installer for a fresh Debian/Ubuntu machine.
#
# Installs gamedist-server as a hardened systemd service behind Caddy (automatic
# HTTPS), writes /etc/gamedist/server.toml and creates the first admin account.
# Safe to re-run: existing configuration is kept unless --force is given.
#
#   sudo ./deploy/install.sh --domain games.example.org --email you@example.org
#
# Run with --help for all options.

set -euo pipefail

# ---------------------------------------------------------------------------
# Release download location. NOTE: no releases have been published yet; adjust
# these to the real repository before relying on the download path. Archives
# are expected to be named gamedist-linux-x86_64.tar.gz with a SHA256SUMS file
# alongside (see .github/workflows/release.yml).
RELEASE_BASE_URL="${GAMEDIST_RELEASE_BASE_URL:-https://github.com/akbargg314/gamedist/releases}"
RAW_BASE_URL="${GAMEDIST_RAW_BASE_URL:-https://raw.githubusercontent.com/gamedist/gamedist/main}"
# ---------------------------------------------------------------------------

BIN_PATH=/usr/local/bin/gamedist-server
CONFIG_DIR=/etc/gamedist
CONFIG_FILE=$CONFIG_DIR/server.toml
DATA_DIR=/var/lib/gamedist
UNIT_FILE=/etc/systemd/system/gamedist-server.service
CADDYFILE=/etc/caddy/Caddyfile
SERVICE_USER=gamedist
LISTEN=127.0.0.1:8080
MARKER="# managed by gamedist install.sh"

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=$(cd "$SCRIPT_DIR/.." && pwd)

DOMAIN=""
EMAIL=""
BINARY=""
VERSION="latest"
REGISTRATION="closed"
SERVER_NAME="GameDist server"
FORCE=0
PROMPT_PASSWORD=0
SKIP_CADDY=0
SKIP_ADMIN=0
ASSUME_YES=0
GENERATED_PASSWORD=""
CLEANUP_DIRS=()

cleanup() {
    local d
    for d in "${CLEANUP_DIRS[@]}"; do
        rm -rf -- "$d"
    done
}
trap cleanup EXIT

usage() {
    cat <<EOF
Usage: sudo $0 [options]

Required (prompted for when missing and a terminal is attached):
  --domain NAME          Public host name, e.g. games.example.org (DNS must point here)
  --email ADDRESS        Email of the first administrator account

Binary source (first match wins):
  --binary PATH          Install this gamedist-server binary
                         (otherwise: build with cargo if available in this checkout,
                          otherwise download a release)
  --version TAG          Release tag to download, e.g. v0.1.0 (default: latest)

Options:
  --registration MODE    open | invite | closed (default: closed)
  --name NAME            Server name shown to users (default: "GameDist server")
  --prompt-password      Prompt for the admin password instead of generating one
  --skip-caddy           Do not install/configure Caddy (bring your own reverse proxy)
  --skip-admin           Do not create an admin account
  --force                Overwrite existing /etc/gamedist/server.toml and Caddyfile
                         (backups are kept next to them)
  -y, --yes              Do not ask for confirmation
  -h, --help             Show this help
EOF
}

log() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need_value() {
    [[ $# -ge 2 && -n "$2" ]] || die "$1 needs a value"
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --domain) need_value "$@"; DOMAIN=$2; shift 2 ;;
            --email) need_value "$@"; EMAIL=$2; shift 2 ;;
            --binary) need_value "$@"; BINARY=$2; shift 2 ;;
            --version) need_value "$@"; VERSION=$2; shift 2 ;;
            --registration) need_value "$@"; REGISTRATION=$2; shift 2 ;;
            --name) need_value "$@"; SERVER_NAME=$2; shift 2 ;;
            --prompt-password) PROMPT_PASSWORD=1; shift ;;
            --skip-caddy) SKIP_CADDY=1; shift ;;
            --skip-admin) SKIP_ADMIN=1; shift ;;
            --force) FORCE=1; shift ;;
            -y|--yes) ASSUME_YES=1; shift ;;
            -h|--help) usage; exit 0 ;;
            *) usage >&2; die "unknown option: $1" ;;
        esac
    done
}

is_tty() { [[ -t 0 ]]; }

prompt_missing() {
    if [[ -z "$DOMAIN" ]]; then
        is_tty || die "--domain is required"
        read -r -p "Public domain (e.g. games.example.org): " DOMAIN
    fi
    if [[ -z "$EMAIL" && $SKIP_ADMIN -eq 0 ]]; then
        is_tty || die "--email is required (or --skip-admin)"
        read -r -p "Admin email: " EMAIL
    fi
}

validate_args() {
    DOMAIN=${DOMAIN#https://}
    DOMAIN=${DOMAIN#http://}
    DOMAIN=${DOMAIN%/}
    [[ "$DOMAIN" =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ && "$DOMAIN" == *.* ]] \
        || die "invalid domain: $DOMAIN"
    if [[ $SKIP_ADMIN -eq 0 ]]; then
        [[ "$EMAIL" =~ ^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$ ]] \
            || die "invalid email: $EMAIL"
    fi
    case "$REGISTRATION" in
        open|invite|closed) ;;
        *) die "--registration must be open, invite or closed" ;;
    esac
    [[ "$SERVER_NAME" != *$'\n'* ]] || die "--name must be a single line"
}

preflight() {
    [[ $EUID -eq 0 ]] || die "run as root (sudo $0 …)"
    command -v apt-get >/dev/null || die "this installer supports Debian/Ubuntu (apt-get) only"
    command -v systemctl >/dev/null || die "systemd is required"
}

confirm() {
    [[ $ASSUME_YES -eq 1 ]] && return 0
    is_tty || return 0
    cat <<EOF

About to install GameDist:
  domain        https://$DOMAIN
  admin         ${EMAIL:-<skipped>}
  registration  $REGISTRATION
  config        $CONFIG_FILE
  data          $DATA_DIR
  reverse proxy $([[ $SKIP_CADDY -eq 1 ]] && echo "none (--skip-caddy)" || echo "Caddy")

EOF
    local answer
    read -r -p "Continue? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]] || die "aborted"
}

apt_install() {
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@"
}

install_prerequisites() {
    log "installing prerequisites"
    apt-get update -q
    apt_install ca-certificates curl tar coreutils
}

# --- binary -----------------------------------------------------------------

install_binary_from() {
    local src=$1
    [[ -f "$src" ]] || die "binary not found: $src"
    "$src" --version >/dev/null 2>&1 || die "$src does not run on this machine"
    install -m 0755 "$src" "$BIN_PATH.new"
    mv -f "$BIN_PATH.new" "$BIN_PATH"
    log "installed $("$BIN_PATH" --version)"
}

build_from_source() {
    log "building gamedist-server from source in $REPO_DIR"
    (cd "$REPO_DIR" && cargo build -p gamedist-server --release --locked)
    install_binary_from "$REPO_DIR/target/release/gamedist-server"
}

download_release() {
    local arch asset url sums tmp
    arch=$(uname -m)
    [[ "$arch" == "x86_64" ]] || die "no prebuilt release for $arch; use --binary or install Rust (cargo) and re-run from a source checkout"
    asset="gamedist-linux-x86_64.tar.gz"
    if [[ "$VERSION" == "latest" ]]; then
        url="$RELEASE_BASE_URL/latest/download"
    else
        url="$RELEASE_BASE_URL/download/$VERSION"
    fi
    tmp=$(mktemp -d)
    CLEANUP_DIRS+=("$tmp")
    log "downloading $url/$asset"
    curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$asset" "$url/$asset" \
        || die "download failed; releases may not exist yet — use --binary PATH or build from source"
    if curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/SHA256SUMS" "$url/SHA256SUMS"; then
        sums=$(grep " $asset\$" "$tmp/SHA256SUMS" || true)
        [[ -n "$sums" ]] || die "SHA256SUMS does not list $asset"
        (cd "$tmp" && printf '%s\n' "$sums" | sha256sum -c --quiet) || die "checksum mismatch for $asset"
    else
        warn "no SHA256SUMS published next to the release; skipping checksum verification"
    fi
    tar -xzf "$tmp/$asset" -C "$tmp"
    local bin
    bin=$(find "$tmp" -type f -name gamedist-server | head -n 1)
    [[ -n "$bin" ]] || die "gamedist-server not found in $asset"
    install_binary_from "$bin"
}

install_binary() {
    if [[ -n "$BINARY" ]]; then
        install_binary_from "$BINARY"
    elif command -v cargo >/dev/null && [[ -f "$REPO_DIR/Cargo.toml" && -d "$REPO_DIR/crates/server" ]]; then
        build_from_source
    else
        download_release
    fi
}

# --- user, directories, config ------------------------------------------------

create_user_and_dirs() {
    if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
        log "creating system user $SERVICE_USER"
        useradd --system --user-group --home-dir "$DATA_DIR" --no-create-home \
            --shell /usr/sbin/nologin "$SERVICE_USER"
    fi
    install -d -m 0750 -o root -g "$SERVICE_USER" "$CONFIG_DIR"
    install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$DATA_DIR"
}

toml_string() {
    local s=$1
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    printf '"%s"' "$s"
}

backup() {
    local file=$1 dest
    dest="$file.bak.$(date +%Y%m%d%H%M%S)"
    cp -a "$file" "$dest"
    log "backed up $file to $dest"
}

write_config() {
    if [[ -e "$CONFIG_FILE" && $FORCE -eq 0 ]]; then
        log "keeping existing $CONFIG_FILE (use --force to overwrite)"
        return
    fi
    [[ -e "$CONFIG_FILE" ]] && backup "$CONFIG_FILE"
    log "writing $CONFIG_FILE"
    local tmp
    tmp=$(mktemp "$CONFIG_DIR/.server.toml.XXXXXX")
    cat >"$tmp" <<EOF
$MARKER
# Reference for every key: deploy/server.toml.example in the GameDist repository.

name = $(toml_string "$SERVER_NAME")
listen = "$LISTEN"
public_url = "https://$DOMAIN"
data_dir = "$DATA_DIR"

# Initial mode only; change it later on the /admin portal page.
registration = "$REGISTRATION"

# Caddy (or another reverse proxy) runs in front of the server.
trust_proxy = true
EOF
    chown root:"$SERVICE_USER" "$tmp"
    chmod 0640 "$tmp"
    mv -f "$tmp" "$CONFIG_FILE"
}

# --- Caddy ----------------------------------------------------------------------

install_caddy() {
    if command -v caddy >/dev/null; then
        return
    fi
    log "installing Caddy"
    if apt-cache show caddy >/dev/null 2>&1 && apt_install caddy; then
        return
    fi
    log "caddy not in the distribution repositories; adding the official Caddy apt repository"
    apt_install gnupg debian-keyring debian-archive-keyring apt-transport-https
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' \
        | gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' \
        >/etc/apt/sources.list.d/caddy-stable.list
    apt-get update -q
    apt_install caddy
}

caddyfile_is_replaceable() {
    [[ ! -e "$CADDYFILE" ]] && return 0
    grep -qF "$MARKER" "$CADDYFILE" && return 0
    # Unmodified file shipped by the caddy package.
    if dpkg -S "$CADDYFILE" >/dev/null 2>&1 && ! dpkg --verify caddy 2>/dev/null | grep -qF "$CADDYFILE"; then
        return 0
    fi
    return 1
}

configure_caddy() {
    [[ $SKIP_CADDY -eq 1 ]] && return
    install_caddy
    if ! caddyfile_is_replaceable && [[ $FORCE -eq 0 ]]; then
        warn "$CADDYFILE has local changes; not touching it (use --force, or add a site block proxying https://$DOMAIN to $LISTEN yourself)"
        return
    fi
    [[ -e "$CADDYFILE" ]] && backup "$CADDYFILE"
    log "writing $CADDYFILE"
    install -d -m 0755 "$(dirname "$CADDYFILE")"
    cat >"$CADDYFILE" <<EOF
$MARKER
$DOMAIN {
	encode zstd gzip

	# Release manifests may be up to 64 MiB, chunk uploads up to 8 MiB.
	request_body {
		max_size 80MB
	}

	reverse_proxy $LISTEN
}
EOF
    chmod 0644 "$CADDYFILE"
    caddy validate --config "$CADDYFILE" --adapter caddyfile >/dev/null \
        || die "generated $CADDYFILE does not validate"
    systemctl enable caddy >/dev/null
    if systemctl is-active --quiet caddy; then
        systemctl reload caddy
    else
        systemctl start caddy
    fi
}

# --- systemd --------------------------------------------------------------------

install_unit() {
    local src="$SCRIPT_DIR/gamedist-server.service" tmp
    tmp=$(mktemp)
    if [[ -f "$src" ]]; then
        cp "$src" "$tmp"
    else
        log "fetching unit file from $RAW_BASE_URL"
        curl -fsSL --proto '=https' -o "$tmp" "$RAW_BASE_URL/deploy/gamedist-server.service" \
            || { rm -f "$tmp"; die "gamedist-server.service not found next to this script"; }
    fi
    install -m 0644 "$tmp" "$UNIT_FILE"
    rm -f "$tmp"
    systemctl daemon-reload
    systemctl enable gamedist-server >/dev/null
    log "(re)starting gamedist-server"
    systemctl restart gamedist-server
}

wait_healthy() {
    local _
    for _ in $(seq 1 30); do
        if curl -fsS "http://$LISTEN/.well-known/gamedist.json" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    systemctl --no-pager status gamedist-server || true
    die "gamedist-server did not become healthy; see: journalctl -u gamedist-server"
}

# --- admin account ----------------------------------------------------------------

generate_password() {
    # 24 URL-safe characters (~144 bits).
    head -c 18 /dev/urandom | base64 | tr '+/' '-_'
}

read_password() {
    local p1 p2
    is_tty || die "--prompt-password needs a terminal"
    while true; do
        read -r -s -p "Admin password (min. 10 characters): " p1; echo
        read -r -s -p "Repeat: " p2; echo
        if [[ "$p1" != "$p2" ]]; then
            warn "passwords do not match"
        elif [[ ${#p1} -lt 10 ]]; then
            warn "password too short"
        else
            break
        fi
    done
    GAMEDIST_PASSWORD=$p1
}

create_admin() {
    [[ $SKIP_ADMIN -eq 1 ]] && return
    local marker="$DATA_DIR/.install-admin-created"
    if [[ -f "$marker" ]]; then
        log "admin account already created by a previous run ($(cat "$marker")); skipping"
        return
    fi
    if [[ $PROMPT_PASSWORD -eq 1 ]]; then
        read_password
    else
        GAMEDIST_PASSWORD=$(generate_password)
        GENERATED_PASSWORD=$GAMEDIST_PASSWORD
    fi
    # Passed through the environment only, never on a command line.
    export GAMEDIST_PASSWORD
    log "creating admin account $EMAIL"
    if runuser -u "$SERVICE_USER" -- "$BIN_PATH" --config "$CONFIG_FILE" user-add --email "$EMAIL" --admin; then
        printf '%s\n' "$EMAIL" >"$marker"
        chown "$SERVICE_USER:$SERVICE_USER" "$marker"
    else
        GENERATED_PASSWORD=""
        warn "could not create $EMAIL (it may already exist); create an admin later with:"
        warn "  sudo -u $SERVICE_USER GAMEDIST_PASSWORD=… $BIN_PATH --config $CONFIG_FILE user-add --email … --admin"
    fi
    unset GAMEDIST_PASSWORD
}

# --- summary ------------------------------------------------------------------------

summary() {
    cat <<EOF

GameDist is installed.

  Server        https://$DOMAIN
  Config        $CONFIG_FILE
  Data          $DATA_DIR   (back this directory up)
  Logs          journalctl -u gamedist-server -f
EOF
    if [[ -n "$GENERATED_PASSWORD" ]]; then
        cat <<EOF

  Admin login   $EMAIL
  Password      $GENERATED_PASSWORD
                (shown only once; store it in a password manager)
EOF
    fi
    cat <<EOF

Next steps:
  1. Make sure DNS for $DOMAIN points here and ports 80/443 are open
     (e.g. ufw allow 80,443/tcp). Caddy obtains the certificate automatically.
  2. Check: curl https://$DOMAIN/.well-known/gamedist.json
  3. Sign in at https://$DOMAIN/login; manage users, invites, registration,
     quotas and takedowns at https://$DOMAIN/admin.
  4. Publish from your machine:
       gamedist login https://$DOMAIN --email ${EMAIL:-you@example.org}
       gamedist init my-game --server https://$DOMAIN
     See docs/src/publish-a-game.md.
EOF
}

main() {
    parse_args "$@"
    preflight
    prompt_missing
    validate_args
    confirm
    install_prerequisites
    install_binary
    create_user_and_dirs
    write_config
    install_unit
    wait_healthy
    configure_caddy
    create_admin
    summary
}

main "$@"
