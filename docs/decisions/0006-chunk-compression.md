# 0006. Per-chunk compression

Status: Accepted

## Context

The plan allowed optional per-chunk `zstd` compression flagged in the
manifest (`"encoding": "zstd"`). Most game assets are already compressed, and
optional encodings complicate every consumer: chunk IDs, size checks, torrents
(which describe raw files) and the web-seed layout.

## Decision

- Protocol v0 chunks are always stored and served as **raw bytes**; the chunk
  ID is the BLAKE3 hash of those bytes.
- Per-chunk `zstd` encoding is **reserved** for a future protocol revision.
  v0 manifests MUST NOT contain an `encoding` member, and the JSON Schemas
  reject it (`"not": { "required": ["encoding"] }`), so consumers reject such
  manifests.
- HTTP transport compression (`Content-Encoding`) is always allowed.

## Consequences

- One code path for chunks everywhere; torrents and web seeds map directly to
  stored chunks.
- Highly compressible builds cost more bandwidth and storage than necessary.
- Adding compression later needs a protocol revision; because v0 consumers
  reject `encoding`, old launchers fail loudly instead of installing garbage.
