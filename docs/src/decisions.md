# Design decisions

Architecture decision records live in
[`docs/decisions/`](https://github.com/akbargg314/gamedist/tree/main/docs/decisions)
in the repository. Each records the context, the decision and its
consequences.

| ADR | Decision |
|---|---|
| [0001](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0001-name-and-uri-scheme.md) | Name GameDist; `gamedist://` URIs, `gamedist+http://` for local development |
| [0002](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0002-torrent-v1.md) | BitTorrent v1 torrents with BEP 19 web seeds and the fixed name `files`; hybrid v2 deferred |
| [0003](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0003-launcher-frontend.md) | Launcher GUI: Tauri 2 + Svelte |
| [0004](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0004-no-standalone-website.md) | No standalone website in v1; the server's SSR portal is enough (superseded by 0010) |
| [0005](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0005-launcher-auto-update.md) | Launcher self-update: protocol first, GitHub Releases fallback (not implemented) |
| [0006](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0006-chunk-compression.md) | Per-chunk zstd reserved; v0 manifests must not use `encoding` |
| [0007](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0007-immutable-multi-platform-releases.md) | A release is immutable and contains all platforms; `push --build` |
| [0008](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0008-sqlite-and-local-storage-first.md) | SQLite and local chunk storage first; Postgres and S3 later |
| [0009](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0009-torrent-client-design.md) | Launcher BitTorrent client: torrents untrusted, data lifted out of a scratch dir chunk by chunk, HTTP fills gaps afterwards, seed only verified installs |
| [0010](https://github.com/akbargg314/gamedist/blob/main/docs/decisions/0010-project-website.md) | Static project website (landing page, downloads, this book) on GitHub Pages; game catalogs stay on the servers |
