# 0008. SQLite and local storage first

Status: Accepted

## Context

The plan lists SQLite (default) and Postgres (optional) databases, and
`local` and `s3` chunk storage backends. Supporting all of them from the start
multiplies the test matrix before the core server is proven.

## Decision

- Implemented: **SQLite** (`sqlx`, WAL mode) at `<data_dir>/gamedist.sqlite`
  and a **local directory** chunk store at `<data_dir>/chunks/`
  (`<id[0..2]>/<id>`, verified writes, atomic rename).
- **Not implemented yet**: Postgres and S3-compatible storage. They remain
  planned; there are no config keys for them.
- Scaling chunk delivery is done with `chunks_url` (a CDN in front of
  `/chunks/`), since chunks are immutable.

## Consequences

- A server is one binary plus one directory; backups are a copy of that
  directory.
- One server process per data directory; no horizontal scaling of the API.
- Disk space is bounded by the local volume; quotas and garbage collection
  keep it in check.
- Adding Postgres/S3 later requires abstracting the database and store
  behind traits; the protocol is unaffected.
