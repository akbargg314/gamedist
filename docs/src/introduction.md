# Introduction

GameDist is an open, decentralized alternative to the distribution layer of
commercial game stores. It has three parts:

- **`gamedist-server`** — a distributor server anyone can run. One binary
  serves the REST API, chunk storage, generated torrents with a small HTTP
  tracker, and a server-rendered web portal for developers and admins.
- **`gamedist`** — the developer CLI. It chunks a build, uploads only what
  the server is missing, signs a release manifest with the developer's key and
  publishes it.
- **`gamedist-launcher`** — the player application (Tauri GUI plus a headless
  mode). It adds games by URL, installs, verifies, launches and delta-updates
  them, over HTTP or BitTorrent, and can seed installed games to other
  players. The engine and headless mode are covered by end-to-end tests; the
  GUI has only been run by hand on Linux and has no automated tests.

There is no central registry. A game lives at `https://<host>/games/<slug>`
on any server; players can also follow `gamedist://<host>/games/<slug>` links.
Servers can optionally expose a catalog that launchers show in "Discover".

## How it fits together

1. The developer runs `gamedist init`, which generates an Ed25519 signing key
   that stays on the developer's machine.
2. `gamedist push` splits every file into content-defined chunks (FastCDC,
   512 KiB–8 MiB), identified by their BLAKE3 hash. The server stores chunks by
   ID, so identical data is stored and downloaded once.
3. The release manifest lists every file and its chunks for every platform,
   and is signed by the developer. The server checks the signature and that
   every chunk is present, then publishes it. Releases are immutable.
4. The launcher fetches the game manifest, pins the developer key on first
   install, verifies the release manifest, downloads the chunks it does not
   already have, verifies each one, and assembles the install tree atomically.

The server is a host, not an authority: it cannot change a game without the
developer's key. See [Security model](security.md).

This documentation is part of the [project website](../index.html), which
also has the [downloads](../download.html).

## Where to start

- Operators: [Run a server](run-a-server.md).
- Developers: [Publish a game](publish-a-game.md).
- Players: [Launcher](launcher.md).
- Implementers: [Protocol](protocol.md).
