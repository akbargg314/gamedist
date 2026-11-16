# 0002. BitTorrent v1 torrents with web seeds

Status: Accepted

## Context

PLAN §9.2 asked whether to generate v1-only or hybrid v1+v2 torrents (hybrid
preferred if the torrent library could create them). Torrents must work with
plain HTTP servers as seeds, and their content must be checkable against the
signed release manifest.

## Decision

- The server generates **BitTorrent v1** (BEP 3) metainfo, one torrent per
  release platform, for the assembled install tree (SPEC §4.5). Generation is
  implemented in `gamedist-core` without an external torrent library.
- Each torrent is multi-file with the fixed `name` **`files`**, files in
  manifest order, and a BEP 19 `url-list` web seed of
  `<host>/games/<slug>/releases/<version>/<platform>/`. A BEP 19 client
  requests `<web seed>/files/<path>`, which is exactly the server's HTTP file
  layout, so every GameDist server (and any static mirror of that layout)
  seeds automatically.
- `announce` is the server's own HTTP tracker (`/announce`) when enabled, plus
  any `external_trackers`.
- Torrents are unsigned; clients verify downloaded data against the signed
  manifest. Info hashes are published in the release list, not in the signed
  manifest.
- Hybrid v2 torrents are **deferred**.

## Consequences

- Maximum client compatibility; one piece hash scheme (SHA-1) to implement.
- v1 piece hashes span file boundaries and are not per-file, so torrent
  pieces and GameDist chunks do not align; per-chunk BLAKE3 verification is
  still required after download.
- Adding v2 later is additive (new torrent files or hybrid metainfo) and does
  not change the signed manifest format.
- Only an HTTP tracker exists; a UDP tracker is not implemented.
- The web seed serves third-party BitTorrent clients. The GameDist launcher's
  own client (librqbit) has no BEP 19 support, so the launcher fills gaps over
  HTTP at the chunk level instead; see
  [ADR 0009](0009-torrent-client-design.md).
