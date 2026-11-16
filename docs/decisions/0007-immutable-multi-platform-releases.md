# 0007. Immutable, multi-platform releases

Status: Accepted

## Context

The plan's CLI pushed one platform at a time
(`gamedist push <dir> --platform …`), while release manifests are signed and
immutable. Adding a platform to an existing version would mean changing a
signed, published manifest.

## Decision

- A release manifest contains **every platform** of its version, and once
  published it never changes (SPEC §4.3).
- `gamedist push` accepts several builds in one run:
  `--build PLATFORM=DIR` (repeatable), in addition to the positional build
  dir with `--platform`.
- Re-publishing the same content is a no-op; publishing the same version with
  different content is rejected with a conflict ("bump the version").

## Consequences

- CI pipelines must collect all platform builds before pushing.
- Adding a platform later requires a new version.
- Mirrors, torrents and launcher pins can rely on a version meaning exactly one
  signed manifest.
