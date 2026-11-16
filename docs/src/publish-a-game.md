# Publish a game

You need the `gamedist` CLI (`cargo install --path crates/cli`, or a release
binary) and an account on a server.

## 1. Create the project and signing key

```sh
cd my-game
gamedist init cool-game --title "Cool Game" --server https://games.example.org
```

This writes `gamedist.toml` and, unless a key with that name already exists,
generates an Ed25519 signing key at `~/.config/gamedist/keys/cool-game.key`
(file mode 0600; the directory is `$GAMEDIST_CONFIG_DIR` if set). Use
`--key NAME` to choose a different key name, e.g. to share one key between
games.

**Back up the key now** (see [Key backup](#key-backup)). Whoever holds it can
publish updates to your game; if you lose it, players have to re-trust a new
key.

The slug must match `[a-z0-9][a-z0-9-]{1,63}`.

## 2. Log in

```sh
gamedist login https://games.example.org --email you@example.org
```

The password is prompted for (or taken from `GAMEDIST_PASSWORD`). Add
`--register` to create the account first, and `--invite CODE` on servers with
invite-only registration. The API token is stored in the OS keyring, or in
`~/.config/gamedist/credentials.toml` (mode 0600) when no keyring is
available.

## 3. Register the game

```sh
gamedist game create --summary "One-line pitch" --description store-page.md
```

This registers your public key with your account and creates the game with
the slug and title from `gamedist.toml`. The server is added to `servers` in
`gamedist.toml` if it was given with `--server`.

## 4. Push a build

```sh
gamedist push build/linux --platform linux-x86_64 --exe CoolGame.x86_64 --version 1.0.0
```

- Without `--platform`, the build is labelled with the platform of the machine
  running the CLI.
- `--exe` sets the launch executable (relative to the build dir) and saves it
  under `[platforms.<platform>]` in `gamedist.toml`, so later pushes do not
  need it.
- `--channel stable` promotes the version right after publishing.
- `--changelog CHANGES.md` embeds release notes.
- `--server URL` pushes to one server instead of all in `gamedist.toml`.
- `--jobs N` sets upload parallelism (default 8).

What happens: every file is chunked and hashed locally, the server is asked
which chunks it lacks (`POST /chunks/missing`), only those are uploaded (with
retries), then the manifest is signed and published. Re-running the same push
uploads nothing and reports "already published". Symlinks in the build
directory are refused.

### Multi-platform releases

A release contains every platform of that version, and releases are
immutable, so push all platforms of a version in one command:

```sh
gamedist push --version 1.1.0 \
  --build windows-x86_64=build/windows \
  --build linux-x86_64=build/linux \
  --build macos-aarch64=build/macos \
  --channel stable
```

`--build` can be combined with a positional build dir and `--platform`. Each
platform needs a launch executable in `gamedist.toml` (or `--exe`, which is
applied to platforms that have none).

## 5. Promote

Channels are pointers to versions. `stable` is the default channel players
follow.

```sh
gamedist promote 1.1.0                 # --channel stable is the default
gamedist promote 1.2.0-beta.1 --channel beta
```

Versions are SemVer 2.0.0 without build metadata.

## 6. Store-page assets

```sh
gamedist assets set --icon icon.png --banner banner.png --screenshots a.png b.png c.png
```

Each image is uploaded as a single chunk, so it must be at most 8 MiB.
`--screenshots` replaces the whole list. Assets are part of the unsigned game
manifest (see [Security model](security.md)).

## Inspecting

```sh
gamedist status              # project, key, game ID, channels and releases per server
gamedist releases list       # same listing
gamedist key show            # public key of the project's key
gamedist verify 1.0.0.json --key ed25519:…   # offline schema + signature check of a release manifest
```

Release manifests can be downloaded from
`https://<host>/games/<slug>/releases/<version>.json`.

## `gamedist.toml` reference

```toml
slug = "cool-game"            # required
title = "Cool Game"           # required
key = "cool-game"             # required: key name in ~/.config/gamedist/keys/
servers = ["https://games.example.org"]   # targets for push/promote/assets

[platforms.windows-x86_64]
launch = { exe = "CoolGame.exe" }

[platforms.linux-x86_64]
launch = { exe = "CoolGame.x86_64", args = ["--fullscreen"], cwd = "." }
```

- `launch.exe` — path of the executable inside the build, `/`-separated.
- `launch.args` — optional argument list.
- `launch.cwd` — optional working directory relative to the install root
  (default `.`).
- Platform names are `<os>-<arch>`: `windows-x86_64`, `linux-x86_64`,
  `linux-aarch64`, `macos-aarch64`, `macos-x86_64`, …

Unknown top-level keys are rejected.

Global CLI options: `-C DIR` (project directory, default `.`), `-q` (quiet).

## CI usage

A typical pipeline needs the signing key and an API token.

```sh
export GAMEDIST_CONFIG_DIR="$RUNNER_TEMP/gamedist"   # keep keys/tokens out of $HOME
printf '%s' "$GAMEDIST_SIGNING_KEY" | gamedist key import cool-game -
gamedist push --version "$VERSION" --build linux-x86_64=dist/linux --build windows-x86_64=dist/windows
```

Two ways to authenticate:

- **`GAMEDIST_TOKEN`** — an API token created on the server's `/dashboard`
  page. When set, it is used for every server and nothing is stored.
- **Login in CI** — `GAMEDIST_PASSWORD=… gamedist login <url> --email …`
  with `GAMEDIST_TOKEN_STORE=file`, so the token goes to the credentials file
  in `GAMEDIST_CONFIG_DIR` instead of an OS keyring (usually unavailable on CI
  runners).

Store the private key (output of `gamedist key export`) and the token as CI
secrets. `gamedist` never prints the token.

## Key backup

```sh
gamedist key export cool-game > cool-game.key.backup   # PRIVATE: treat like a password
gamedist key import cool-game cool-game.key.backup     # on a new machine
```

Keep the backup offline or in a password manager. `key import` refuses to
overwrite an existing key with the same name. To replace a key on purpose,
see [Key rotation](key-rotation.md).

## Why releases are immutable

A published version can never change. Pushing the same version with
different content fails with "releases are immutable, bump the version".

- Launchers pin a version's signed manifest; mutable versions would make
  "verified 1.2.0" meaningless and break delta updates and torrents, whose
  info hashes are derived from the content.
- Mirrors carry byte-identical signed manifests; a mutable version could
  diverge between hosts.
- Rollback protection relies on versions being ordered and fixed.

To fix a bad release, publish a new version and promote it. You can unpublish
a version from the dashboard; its chunks are garbage-collected later. Do not
reuse an unpublished version number for different content: players and
mirrors may already have the old one.
