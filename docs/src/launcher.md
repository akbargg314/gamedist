# Launcher

The launcher is the player's side of GameDist. It has two front ends over one
engine:

- **The engine and headless CLI** (`crates/launcher`, binary
  `gamedist-launcher`). This is where adding, installing, updating, verifying,
  launching, BitTorrent and key pinning are implemented. It is covered by the
  end-to-end tests in `tests/e2e`.
- **The desktop app** (`apps/launcher`): a Tauri 2 shell with a Svelte 5
  frontend and a Steam-like layout (Library sidebar and grid, Discover,
  Downloads, Settings). The frontend builds and type-checks; the Rust shell
  (`apps/launcher/src-tauri`) compiles and has been run by hand on Linux, but
  has no automated tests and has never been built on Windows or macOS, so
  treat the GUI as largely untested. See
  [`apps/launcher/README.md`](https://github.com/akbargg314/gamedist/blob/main/apps/launcher/README.md) for building it.

Everything below applies to both front ends, because the GUI calls the same
engine. Commands are shown for the headless CLI.

## Getting the headless launcher

From a checkout of the repository:

```sh
cargo build -p gamedist-launcher --release
target/release/gamedist-launcher --headless --help
```

The standalone binary only runs with `--headless` (or `--help` /
`--version`); without it, it exits with status 2 and points you at the GUI.
The GUI binary, which is also called `gamedist-launcher`, hands every
invocation that contains `--headless` to the same code.

## Data directory

All launcher state lives in one data directory: by default `gamedist` inside
the per-user data directory (`~/.local/share/gamedist` on Linux). Override it
with `--data-dir DIR` or the `GAMEDIST_DATA_DIR` environment variable. Two
different data directories behave like two different players.

| Path | Contents |
|---|---|
| `library.json` | Settings, library entries (with the pinned developer key per game) and catalog servers |
| `manifests/<game id>/<version>.json` | Signed release manifest of each installed version |
| `store/` | Content store of downloaded chunks (`<id[0..2]>/<id>`) |
| `torrent/` | BitTorrent scratch space |

If `library.json` cannot be parsed, the launcher stops with an error instead
of overwriting it, because it holds your pinned keys.

Each install directory also gets an `installed.json` marker with the game ID,
slug, version, platform, pinned developer key and install path.

## Adding a game

```sh
gamedist-launcher --headless add https://games.example.org/games/cool-game
```

Accepted forms:

- `https://games.example.org/games/cool-game`
- `gamedist://games.example.org/games/cool-game` (the GUI registers itself as
  the handler for `gamedist://` links)
- `gamedist+http://127.0.0.1:8080/games/cool-game`: plain HTTP, for local
  development only

The launcher reads the server info and the game manifest and checks that the
Game ID equals the one derived from the developer key and slug. On first
sight of a game it **pins the developer key** (trust on first use) and prints
it; compare it with the key the developer publishes.

Adding the same game from a second host (same developer key and slug, so the
same Game ID) does not create a second entry. The host is added as an extra
source of the existing entry, and installs and updates try the sources in
order and fetch chunks from all of them. See
[Mirroring and federation](mirroring-and-federation.md).

## Installing

```sh
gamedist-launcher --headless install https://games.example.org/games/cool-game --dir ~/Games/cool-game
```

`install` accepts a URL (the game is added first if needed) or the slug or
Game ID of a game already in the library. Options:

| Option | Meaning |
|---|---|
| `--dir DIR` | Install directory. Default: `<install root>/<slug>`, or the existing install's directory |
| `--platform NAME` | Install this platform's build (for example `windows-x86_64`) |
| `--channel NAME` | Follow this channel from now on (default `stable`) |
| `--version VERSION` | Install exactly this version instead of the channel's |
| `--allow-downgrade` | Permit moving to a lower version than the installed one |
| `--accept-key-change` | Trust a changed developer key (see [below](#the-key-change-warning)) |

What happens:

1. The channel is resolved to a version and the signed release manifest is
   fetched and verified against the **pinned** key.
2. The platform is the machine's own (for example `linux-x86_64`). On Linux
   and macOS, if there is no native build but a Windows build exists and a
   Wine-compatible runner is available, the Windows build is chosen.
3. Chunks that are already in the content store, or that can be recovered
   from the currently installed files, are not downloaded. The rest are
   fetched with the selected [transport](#transports-and-bittorrent). Every
   chunk is verified against its BLAKE3 ID before it is stored.
4. Files are assembled and swapped into place atomically, with a journal so
   that a failed or interrupted install is rolled back (an interrupted one on
   the next run).
5. The signed manifest and `installed.json` are written only after the files
   are in place.
6. Unless `keep_chunks` is on, the chunks used for the install are removed
   from the content store afterwards.

The launcher refuses to install into a non-empty directory that is not
already this game's install, and into a directory that holds another game.

## Updating

```sh
gamedist-launcher --headless check cool-game     # what an update would download
gamedist-launcher --headless update cool-game    # apply it
```

`check` reports the installed and latest versions and how many bytes the
update would download. `update` moves the install to the current version of
its channel. An update:

- is verified against the pinned key, like an install;
- is refused if the channel points at a lower version than the installed one,
  unless you pass `--allow-downgrade`. Channel pointers are not signed, so this
  stops a server from rolling players back to an old, validly signed release;
- downloads only the chunks that changed. Chunks that are still in the
  installed files are reused ("harvested") even when they moved within a
  file, thanks to content-defined chunking;
- deletes files that the new version no longer contains, and leaves files the
  game created itself (save games, for example) alone;
- is applied with the same atomic swap and rollback as an install.

## Verifying and repairing

```sh
gamedist-launcher --headless verify cool-game              # re-hash and repair
gamedist-launcher --headless verify cool-game --no-repair  # only report
```

`verify` re-hashes every installed file against the signed manifest. Damaged
or missing files are rebuilt; good chunks inside damaged files are reused,
so only the damaged parts are downloaded. With `--no-repair` the damaged files
are listed and the exit status is 2.

## Launching

```sh
gamedist-launcher --headless launch cool-game -- --windowed
```

The launcher starts the executable named in the signed manifest, with the
manifest's arguments, then the arguments after `--`, in the manifest's
working directory. It waits for the game to exit and returns the game's exit
code.

Windows builds on Linux or macOS are started through a Wine-compatible
runner: the one set with `settings --wine PATH`, otherwise `wine` from
`PATH`. If a runner is configured but the file does not exist, no runner is
used. Proton and Wine-GE are not detected automatically; point `--wine` at
the runner you want.

## Uninstalling

```sh
gamedist-launcher --headless uninstall cool-game            # remove the files, keep the entry and pin
gamedist-launcher --headless uninstall cool-game --forget   # also forget the game and its pinned key
```

Only files listed in the installed manifest are removed; files the game
created are left in place.

## Listing

```sh
gamedist-launcher --headless list
```

prints every library entry with its Game ID, slug, and installed version,
platform and directory.

## The key-change warning

If one of a game's sources (a host you added it from) presents a **different
developer key** for the same slug, and no valid chain of signed key-rotation
statements links the pinned key to the new one, the launcher stops with a
`DEVELOPER KEY CHANGED` error that names the pinned and the presented key.
Nothing is changed on disk. This applies to `add`, `install`, `update` and
`check`; the headless CLI exits with status 1.

A host you have *not* added the game from is different: the same slug under
another key there is simply a different game with a different Game ID, and
adding it creates a separate library entry. Use the Game ID or the full URL
to tell such entries apart.

The change means one of:

- the developer moved to a new key without publishing a rotation statement,
  or lost the old key (see [Key rotation](key-rotation.md));
- the server, or someone who took it over, is serving a game the original
  developer did not sign.

The launcher cannot tell these apart. Only if you have confirmed the new key
with the developer through another channel (their website, a signed
announcement), accept it explicitly:

```sh
gamedist-launcher --headless add https://games.example.org/games/cool-game --accept-key-change
# or
gamedist-launcher --headless install https://games.example.org/games/cool-game --accept-key-change
```

Accepting re-pins the game to the new key. Because the Game ID is derived
from the key, the entry gets a new Game ID; the existing install is carried
over, so the next update is still a delta. Other sources of the game are
dropped and are added again when they present the new key. `update` has no
`--accept-key-change`; use `add` or `install` with the game's URL.

In the GUI the same situation opens a blocking warning whose default is
Cancel; only an explicit "I trust the new key" re-pins.

If the developer published a valid rotation chain signed by the pinned key,
the launcher follows it without asking: the pin moves to the new key during
the next `install`, `update` or `check`, and the install is carried over.

## Transports and BitTorrent

The `transport` setting chooses how missing chunks are fetched:

| Transport | Behaviour |
|---|---|
| `direct` (default) | HTTP chunk downloads from the game's sources only |
| `torrent-direct` | BitTorrent first; HTTP fetches whatever the swarm did not deliver |
| `torrent` | BitTorrent only; the install fails if the swarm cannot deliver everything |

```sh
gamedist-launcher --headless settings --transport torrent-direct
```

BitTorrent support is the `torrent` cargo feature of `gamedist-launcher`
(on by default, built on librqbit). In a build without it, `torrent-direct`
behaves like `direct` and `torrent` installs fail.

How a torrent install works:

1. After a release is published, the server generates one torrent per
   platform. The launcher fetches it from the host it is installing from; a
   torrent URL that points at a different host is refused. Torrents are
   generated in the background, so right after a release there may be none
   yet: `torrent-direct` then uses HTTP, `torrent` fails.
2. The torrent is **not signed**, so it is accepted only if its info
   dictionary contains exactly `files`, `name` (which must be `files`),
   `piece length` and `pieces`, and its file list (paths, order and sizes)
   is identical to the signed manifest's.
3. The files to be written are downloaded into a scratch directory under
   `<data dir>/torrent/`, never into the install directory. The torrent phase
   ends when the download is complete or when it has made no progress for
   `torrent_stall_secs` seconds (default 20).
4. Chunks are then lifted out of the scratch files one by one, and each is
   kept only if it matches its BLAKE3 ID from the signed manifest. The
   scratch directory is deleted.
5. Whatever is still missing is fetched over HTTP (`torrent-direct`), or the
   install fails (`torrent`).

HTTP therefore fills the gaps chunk by chunk after the torrent phase, not
piece by piece while it runs: librqbit has no web-seed (BEP 19) client.

Peers are found only through the trackers listed in the torrent (normally the
server's own tracker at `/announce`). DHT and local peer discovery are off,
and UPnP port forwarding is not used. The client listens on TCP; set
`--listen-port` and open that port if other players should be able to
connect to you.

The bandwidth limit applies to both HTTP and BitTorrent downloads. The
BitTorrent session reads the bandwidth limit, upload limit and listen port
when it starts (on first use in a process), so changes take effect the next
time the launcher starts.

## Seeding

With seeding on, the launcher shares installed games with other players. An
install directory is seeded only after every file has been verified against
the signed manifest **and** the SHA-1 piece hashes recomputed from those
files equal the torrent's. Because the torrent client then finds the data
complete, it never writes into an install directory. A damaged or modified
install is not seeded. Seeding of a game stops before it is updated,
repaired or uninstalled, and resumes afterwards with the new version's
torrent.

```sh
gamedist-launcher --headless seed
```

`seed` turns the `seed` setting on, seeds every installed game that passes
the checks above (printing a warning for each one that does not), prints the
listen port and keeps running until you press Ctrl-C. It then prints how many
bytes were uploaded per game. It fails if no game could be seeded.

When the `seed` setting is on, `install`, `update` and a repairing `verify`
also start seeding the game afterwards, but only for as long as that process
runs; a headless `install` exits right away, so use `seed` for a long-running
seeder. The GUI resumes seeding every installed game when it starts (if the
`seed` setting is on), seeds games it installs or repairs while it is open,
and has per-game Seed / Stop seeding buttons; seeding stops when the GUI
exits.

## Catalogs and Discover

```sh
gamedist-launcher --headless catalog add https://games.example.org
gamedist-launcher --headless catalog list
gamedist-launcher --headless discover puzzle
gamedist-launcher --headless catalog remove https://games.example.org
```

`catalog add` is refused for a server that does not publish a catalog.
`discover [QUERY]` searches every configured catalog and prints each match's
game URL, title and server; unreachable servers are reported as warnings and
skipped. Pass a game URL from the results to `add` or `install`. Catalogs are
opt-in; there is no default registry.

## Settings

```sh
gamedist-launcher --headless settings                         # print the current settings
gamedist-launcher --headless settings --bandwidth-limit 5000000 --keep-chunks true
```

`settings` always prints the resulting settings as JSON.

| Flag | Setting | Default | Meaning |
|---|---|---|---|
| `--install-root DIR` | `install_root` | `<user data dir>/gamedist/games` | Where games go when no `--dir` is given |
| `--bandwidth-limit N` | `bandwidth_limit` | unlimited | Download limit in bytes per second (HTTP and BitTorrent); `0` = unlimited |
| `--concurrency N` | `concurrency` | `8` | Parallel HTTP chunk downloads (1 to 64) |
| `--keep-chunks true\|false` | `keep_chunks` | `false` | Keep chunks in the content store after installing: faster re-installs and sharing between mirrors, at the cost of disk space |
| `--wine PATH` | `wine` | unset | Wine-compatible runner for Windows builds |
| `--transport T` | `transport` | `direct` | `direct`, `torrent-direct` or `torrent` |
| `--seed true\|false` | `seed` | `false` | Seed installed games |
| `--upload-limit N` | `upload_limit` | unlimited | BitTorrent upload limit in bytes per second; `0` = unlimited |
| `--listen-port N` | `listen_port` | `0` | BitTorrent TCP listen port; `0` = any free port |
| (none) | `torrent_stall_secs` | `20` | Seconds without torrent progress before the torrent phase ends |

`torrent_stall_secs` has no command-line flag; it can be changed in the
`settings` object of `library.json` (while the launcher is not running).

The default install root is derived from the per-user data directory, not
from `--data-dir`, so set `--install-root` when you use a separate data
directory and want its games kept apart.

## Global options and exit codes

| Option | Meaning |
|---|---|
| `--headless` | Run without the GUI |
| `--data-dir DIR` | Data directory (or `GAMEDIST_DATA_DIR`) |
| `--json` | Print results as JSON on standard output |
| `--quiet`, `-q` | No progress output (progress goes to standard error) |

| Exit status | Meaning |
|---|---|
| `0` | Success |
| `1` | Error, including a refused key change or downgrade |
| `2` | `verify --no-repair` found damaged files; also invalid command-line usage |
| other | `launch` returns the game's own exit code |
