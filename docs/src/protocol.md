# Protocol

The authoritative specification of protocol `gamedist/0` is
[`protocol/SPEC.md`](https://github.com/akbargg314/gamedist/blob/main/protocol/SPEC.md) in the repository. It defines:

- identifiers: slugs, Game URLs, `gamedist://` URIs, Game IDs, chunk IDs,
  versions, platforms, channels (§1);
- content-defined chunking (§2);
- canonical JSON (RFC 8785, integers only) and Ed25519 signatures, and the
  trust model (§3);
- the objects: server info, game manifest, release manifest, key-rotation
  statement, torrents, web-seed file layout (§4);
- the REST API under `/api/v0` (§5);
- the client download procedure (§6).

JSON Schemas (draft 2020-12, used by `gamedist-core` for validation):

- [`server-info.schema.json`](https://github.com/akbargg314/gamedist/blob/main/protocol/schemas/server-info.schema.json)
- [`game-manifest.schema.json`](https://github.com/akbargg314/gamedist/blob/main/protocol/schemas/game-manifest.schema.json)
- [`release-manifest.schema.json`](https://github.com/akbargg314/gamedist/blob/main/protocol/schemas/release-manifest.schema.json)
- [`key-rotation.schema.json`](https://github.com/akbargg314/gamedist/blob/main/protocol/schemas/key-rotation.schema.json)

Fixtures, including deliberately invalid ones, are in
[`protocol/fixtures/`](https://github.com/akbargg314/gamedist/tree/main/protocol/fixtures).

Quick map of public URLs on a server:

| URL | Content |
|---|---|
| `/.well-known/gamedist.json` | Server info |
| `/games/<slug>` | Human game page (portal); the API form is `/api/v0/games/<slug>` |
| `/games/<slug>/manifest.json` | Game manifest (unsigned) |
| `/games/<slug>/releases/` | Release list with channels and torrent info hashes |
| `/games/<slug>/releases/<version>.json` | Signed release manifest |
| `/games/<slug>/releases/<version>-<platform>.torrent` | Torrent |
| `/games/<slug>/releases/<version>/<platform>/files/<path>` | Web-seed file layout |
| `/chunks/<id>` | Chunk |
| `/announce` | HTTP tracker |
| `/api/v0/export.jsonl` | Catalog export |

Protocol changes are made in the spec and schemas first; design decisions are
recorded in [Design decisions](decisions.md).
