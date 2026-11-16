# GameDist Launcher

The player-facing desktop app: a Steam-like library, installer, updater and
launcher for games published on any GameDist server. MIT licensed.

* **Shell:** Tauri 2 (`src-tauri/`), wrapping the `gamedist-launcher` engine
  crate (`../../crates/launcher`).
* **UI:** Svelte 5 + TypeScript + Vite (`src/`).

> **Build status:** the Rust shell (`src-tauri/`) compiles (`cargo check` /
> `npm run tauri dev`) and has been run on Linux. The frontend is checked
> and built (`npm run check`, `npm run build`) and also runs against the
> in-memory mock outside Tauri.

```
apps/launcher/
├── index.html, vite.config.ts, svelte.config.js, tsconfig.json, package.json
├── src/
│   ├── App.svelte              layout: top nav, sidebar, main pane
│   ├── app.css                 theme tokens (dark default, light via prefers-color-scheme)
│   ├── components/             GamePage, LibraryGrid, Discover, Downloads, Settings, dialogs…
│   └── lib/
│       ├── api.ts              the ONLY gateway to the backend (Tauri invoke / events)
│       ├── mock.ts             in-memory mock backend used outside Tauri
│       ├── types.ts            wire types mirrored from the Rust crate
│       ├── markdown.ts         small, escape-first Markdown renderer
│       ├── throughput.ts       transfer-rate math for the Downloads view (no Svelte)
│       └── state.svelte.ts     app state (Svelte 5 runes)
├── scripts/gen-icons.ts        generates src-tauri/icons/* and public/favicon.png
├── scripts/check-throughput.ts assertions for throughput.ts (`deno run scripts/check-throughput.ts`)
└── src-tauri/                  Tauri shell (its OWN cargo workspace)
    ├── Cargo.toml, build.rs, tauri.conf.json
    ├── capabilities/default.json
    ├── icons/
    └── src/main.rs, src/commands.rs
```

## `src-tauri` is a separate cargo workspace

The Tauri shell links against the system webview (WebKitGTK on Linux), which
CI machines and server builds usually do not have. It is therefore excluded
from the root workspace (`exclude = ["apps/launcher/src-tauri"]` in the root
`Cargo.toml`) and declares its own empty `[workspace]`. It has its own
`Cargo.lock` and `target/`. Run cargo commands for it from
`apps/launcher/src-tauri/` (or via `npm run tauri …`), never from the repo
root.

All the real logic (downloads, verification, key pinning, install/update,
BitTorrent transport and seeding, headless CLI) lives in `crates/launcher` and
`crates/core`, which are members of the root workspace and build and test
there without any GUI libraries. The engine's default cargo feature `torrent`
(librqbit) is on; `src-tauri` uses it through a plain path dependency.

## Prerequisites

All platforms: [Rust](https://rustup.rs) (stable, 1.90+ for the deep-link
plugin) and Node.js 20+ with npm (or Deno 2, see below).

**Linux** (Debian/Ubuntu names; see <https://v2.tauri.app/start/prerequisites/>):

```sh
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Fedora: `webkit2gtk4.1-devel openssl-devel libappindicator-gtk3-devel librsvg2-devel`
plus the "C Development Tools and Libraries" group.
Arch: `webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module libappindicator-gtk3 librsvg`.

**macOS:** Xcode Command Line Tools (`xcode-select --install`).

**Windows:** Microsoft C++ Build Tools ("Desktop development with C++") and
the WebView2 runtime (preinstalled on Windows 10 1803+ / 11).

## Develop

```sh
cd apps/launcher
npm install
npm run tauri dev        # starts Vite on :5173 and the Tauri window
```

UI-only work needs no Rust at all: `npm run dev` and open
<http://localhost:5173>. Outside Tauri, `src/lib/api.ts` switches to an
in-memory mock backend with a few fictional games (a "demo" badge shows next
to the logo). Useful mock inputs:

* Add game → `gamedist://indie.example.invalid/games/lantern-hollow`
* Key-change warning → `gamedist://evil.example/games/starfall-courier`
* Deep-link simulation → `http://localhost:5173/?open=gamedist://indie.example.invalid/games/lantern-hollow`
* Downloads view → install Tidewalker (3.2 GiB) or Lantern Hollow (410 MiB,
  add it first): with the default transport "torrent + direct" the mock
  downloads ~70 % from peers (source `peers`, the torrent's total), stalls,
  then fetches the rest from the server (source `http`, its own smaller total,
  with one failed attempt that is un-counted), then writes the files, all at a
  wobbling speed. With "torrent" only, peers deliver everything; with
  "direct", there is no peers phase at all. Updating Starfall Courier first
  reads reusable data from the installed files (`harvest`, bytes). Installing
  into a folder under `/mnt/usb` (a fake, nearly full 16 GiB stick) shows the
  "not enough space" warning.
* Verify files → checks the installed bytes (several "workers", some reports
  delivered out of order). Moss & Mortar always "finds" two damaged files and
  repairs them: harvest, HTTP download, write.

Game art is loaded from remote servers and the mock's servers do not exist,
so you will see the generated letter placeholders — the same thing users see
when a server's image fails to load.

Checks:

```sh
npm run check            # svelte-check (TypeScript + Svelte diagnostics)
npm run build            # production frontend into dist/
deno run scripts/check-throughput.ts   # rate math assertions (no permissions needed)
```

With Deno instead of Node: `deno install`, then `deno task check`,
`deno task build`, `deno task dev`.

## Build a release

```sh
cd apps/launcher
npm install
npm run tauri build      # runs `npm run build`, then bundles
```

Bundles land in `src-tauri/target/release/bundle/` (`.deb`, `.rpm`,
`.AppImage` on Linux; `.msi`/NSIS `.exe` on Windows; `.app`/`.dmg` on macOS).
The binary is called `gamedist-launcher`.

Icons in `src-tauri/icons/` are generated by `npm run icons` (a dependency-free
Deno script). To use real artwork instead: `npm run tauri icon path/to/icon-1024.png`.

## Seeding

With *Settings → Seeding → Seed installed games* on, the launcher shares the
installed games over BitTorrent. Before a game is seeded the engine re-hashes
it against the signed manifest and only uses the server's torrent if its piece
hashes match the verified files.

* On startup the shell resumes seeding in a background task (a no-op when
  seeding is off). Games that cannot be seeded show up as non-blocking error
  toasts. Turning seeding on in Settings runs the same step again; turning it
  off stops every seed.
* Installs, updates, repairs and uninstalls stop and restart seeding inside the
  engine; the UI does nothing extra.
* The game page shows "Seeding · uploaded X (ratio Y)" and a Seed / Stop
  seeding button. The Downloads view has a Seeding section with every seeded
  game, its upload counter, the current upload rate and the listen port.
  `seeding_status` is polled every 3 s while the window is visible and
  seeding is on in Settings (also while another view is shown, so the upload
  graph has history); with seeding off it is only polled while the game page
  or the Downloads view is on screen.
* Settings has the upload limit and the TCP listen port (blank/0 = any free
  port). The torrent session starts once per process, so port and limit
  changes apply after a restart if it is already running.
* A finished download reports "N from peers" when BitTorrent contributed.
* On exit (`RunEvent::Exit`, i.e. last window closed or quit) the shell awaits
  the engine's `shutdown()` for up to 5 s so the torrent session stops cleanly.

## Downloads view

`src/components/Downloads.svelte` is the transfer monitor:

* **Graph** (`ThroughputGraph.svelte`, plain SVG): the last 1 or 5 minutes,
  one point per second. Network download is the filled area; its top edge is
  the total download speed. When other players contributed in the visible
  range (or a download from peers is running) the area is stacked: bytes from
  peers at the bottom with a diagonal hatch, bytes from the server above in a
  plain fill, so the two differ without relying on colour. Writing files
  (`assemble`) is the solid line, reading and checking installed files
  (`verify`, `harvest`) the dotted line (only drawn when it happens), upload
  to peers the dashed line when seeding. Hover, or focus it and use the arrow
  keys, for the values of one second (total, server, peers, writes, reads,
  upload).
* **Readouts**: smoothed download speed (4 s moving average) split into
  server / other players when peers are involved; a "From other players"
  tile with the current peers speed and its share of the bytes downloaded by
  the running operation(s) and by the session (hidden for server-only
  downloads, so there is never a "0 % from peers"); peak and average of the
  current downloads (of the session when idle); time left in the current step
  (download, checking files, reusing installed files or writing files; shown
  only after the step has run a few seconds and data is flowing); data
  downloaded this session; disk speed (writing and reading separately); upload
  rate / total shared.
* **Per operation**: what it is doing in plain words, including where it
  downloads from ("from other players (BitTorrent)", "the rest from the
  server", "the damaged parts from the server"), or "reading and checking the
  installed files" / "reading reusable data from the installed version";
  bytes and percent of the step; current speed with what it measures; time
  left; peak and average while downloading; bytes and share from peers when
  there were any. "Waiting for other players" / "Waiting for the server" when
  a download has moved nothing for 5 s.
* **Storage** (`DiskUsage.svelte`): free / total space of the install root and
  of the folder of each running install if it is on another filesystem, the
  space the running installs still need, and a warning when that does not
  fit. Installed games are listed with `installed.size` (from the signed
  manifest) and a total; a size of 0 means unknown (manifest unreadable) and
  shows as "—". The same size is on the game page ("Size on disk") and the
  library grid.
* Idle (nothing moved in the visible range): a session summary instead of an
  empty graph.

How rates are derived (`src/lib/throughput.ts`): progress events carry
cumulative byte counters per phase and, while downloading, per source. The
delta between two events is spread evenly over the time between them (at most
5 s) and binned into 1 s buckets; the phase decides the channel (`download` =
network, `assemble` = disk write, `harvest`/`verify` = disk read, `done` =
nothing), and download bytes with source `peers` are also counted in a
`peers` channel (a subset of network). A change of phase or of source starts
a new counter (the shell always delivers the first event after either). A
download counter that goes backwards (bytes of a failed HTTP attempt or a
failed torrent piece are un-counted) is rebased without counting anything; a
disk counter that goes backwards is a report from a worker thread delivered
after a later one, and is ignored (the displayed value keeps the higher one),
so bursts of near-simultaneous reports count every byte exactly once.
(A stale download report cannot be told apart from an un-count; the shell's
100 ms throttle makes it rare and the error is at most one interval's bytes.)
Rates are moving averages ending *now*, so they fall to zero when events
stop. Upload comes from deltas of the polled `uploaded_bytes`. The sampler
(`AppState.meter`) runs a 1 s timer only while an operation runs, something
is being seeded, rates are still decaying or the Downloads view is open;
otherwise it costs nothing.

## Headless mode

The same binary runs without a window when `--headless` is present (handled
before Tauri starts; implemented in `crates/launcher/src/headless.rs`):

```sh
gamedist-launcher --headless install https://host.example/games/cool-game --dir ~/Games/cool-game
gamedist-launcher --headless check cool-game
gamedist-launcher --headless update cool-game
gamedist-launcher --headless verify cool-game
gamedist-launcher --headless launch cool-game -- --windowed
gamedist-launcher --headless list --json
gamedist-launcher --headless catalog add https://host.example
gamedist-launcher --headless discover "puzzle"
gamedist-launcher --headless --help
```

Global options: `--data-dir <dir>` (or `GAMEDIST_DATA_DIR`), `--json`,
`--quiet`. A changed developer key aborts with an error unless
`--accept-key-change` is passed. The GUI and headless mode share the same data
directory (library, key pins, chunk store).

On Windows, release builds use the GUI subsystem, so headless output is not
attached to the console; redirect it (`… > out.txt 2>&1`) or use a debug
build. Exit codes work either way. The engine crate also builds a standalone
`gamedist-launcher` CLI (`cargo run -p gamedist-launcher -- …` from the repo
root) that needs no webview at all.

## `gamedist://` links

The app handles `gamedist://<host>/games/<slug>` (HTTPS) and
`gamedist+http://<host>/games/<slug>` (plain HTTP, for local development).
Opening one shows the Add-game dialog for that URL; nothing is installed
without confirmation. The schemes are declared in `tauri.conf.json` under
`plugins.deep-link.desktop.schemes`.

When the app is already running, a second launch (which is how Windows and
Linux deliver links) is caught by `tauri-plugin-single-instance` (with its
`deep-link` feature): the URL is forwarded to the running window and the
second process exits.

How the handler gets registered:

* **Windows:** the MSI/NSIS installer writes
  `HKCU\Software\Classes\gamedist` (and `gamedist+http`) pointing at the
  installed exe. Debug builds also register at runtime on startup
  (`register_all`), pointing at the dev binary.
* **macOS:** registration is static, via `CFBundleURLTypes` in the bundle's
  `Info.plist`, generated from the config. It only works for the bundled
  `.app` (best installed in `/Applications`); `tauri dev` cannot receive links.
  macOS delivers links to the running app directly (no second process).
* **Linux:** the `.deb`/`.rpm` install a `.desktop` file with
  `MimeType=x-scheme-handler/gamedist;x-scheme-handler/gamedist+http;`. The
  app also calls `register_all` on every start, which writes a `.desktop` entry
  to `~/.local/share/applications` and runs `xdg-mime default`. That makes
  `tauri dev` and AppImages work. Moving an AppImage breaks the registration
  until it is started again.

Test it with `xdg-open 'gamedist://host.example/games/cool-game'` (Linux),
`start gamedist://host.example/games/cool-game` (Windows) or
`open gamedist://host.example/games/cool-game` (macOS).

## Backend interface

The frontend calls these commands (see `src-tauri/src/commands.rs`):
`get_settings`, `update_settings`, `list_games` (entries carry
`installed.size`, bytes from the signed manifest, 0 = unknown), `find_game`, `add_game`,
`check_update`, `install_game` (also updates), `verify_game`, `launch_game`,
`running_games`, `uninstall_game`, `start_seeding`, `seed_game`,
`stop_seeding`, `seeding_status` (`{ games: SeedStatus[], port }`),
`take_seed_failures`, `disk_space`, `list_catalogs`, `add_catalog`,
`remove_catalog`, `discover`, `take_pending_links`.

`disk_space` (`{ path }`) returns `{ path, total_bytes, free_bytes, volume }`
for the filesystem holding `path`. The path need not exist: the command walks
up to the nearest existing directory and reports which one it measured.
`free_bytes` is the space available to the user; `volume` is an opaque
filesystem id (device number on Unix, drive or share on Windows) so the UI can
tell whether two folders share a disk. Implemented with the `fs4` crate
(MIT OR Apache-2.0; rustix / windows-sys underneath).

`update_settings` takes a **patch** (`{ patch: { field: value, … } }`): only
the fields present are changed, so settings the UI does not edit (such as
`torrent_stall_secs`) are kept. `null` is a value (e.g. "unlimited"), not
"unchanged".

Errors are `{ kind: "key_changed" | "error", message, pinned?, presented?, slug?, source_url? }`.
On `key_changed` the UI shows a blocking warning that defaults to Cancel.
Only an explicit "I trust the new key" re-calls `add_game` with
`accept_key_change = true`.

Events: `gamedist://progress` `{ game_id, op, phase, source, done, total }`
(throttled to ~10/s; phase and source changes and completion are always
delivered). `done`/`total` are bytes of the phase: `verify` = bytes checked of
the installed size, `harvest` = bytes read from the installed files,
`download` = bytes of one `source` (`"peers"`: the torrent's totals, then
`"http"`: its own total for the rest; `null` outside `download`),
`assemble` = bytes written. A repair (`op: "verify"`) reports `verify`, then
`harvest`, `download` and `assemble`. `gamedist://exited` `{ game_id, code }`,
`gamedist://open-url` `[url…]`, `gamedist://seed-failures`
`[{ slug, reason }…]` (startup seeding failures that arrive after the UI
called `take_seed_failures`).

## Security notes

* CSP: `default-src 'self'; img-src 'self' https: http: data:; style-src 'self' 'unsafe-inline'; connect-src ipc: http://ipc.localhost`.
  The webview never talks to game servers directly (except for images). All
  network access goes through the Rust engine.
* Descriptions and changelogs come from untrusted servers. `src/lib/markdown.ts`
  escapes everything first and emits only a fixed set of tags. Links are
  limited to http(s)/mailto and open in the system browser. Images are not
  rendered, to avoid tracking pixels.
* The opener capability only allows `http(s)://` and `mailto:` URLs.
