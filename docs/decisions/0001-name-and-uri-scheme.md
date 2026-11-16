# 0001. Name and URI scheme

Status: Accepted

## Context

The plan used "GameDist" as a working name and asked for a final project name
and a URI scheme for one-click installs (PLAN §9.1). Names appear in the
protocol identifier, URIs, binaries, config paths and keyring entries, so they
are expensive to change later.

## Decision

- Project name: **GameDist**. Binaries `gamedist` (CLI), `gamedist-server`,
  `gamedist-launcher`; protocol identifier `gamedist/0`; server info at
  `/.well-known/gamedist.json`.
- Install URI: `gamedist://<host>/games/<slug>`, equivalent to
  `https://<host>/games/<slug>`.
- `gamedist+http://<host>/games/<slug>` maps to plain `http://` and exists
  only for local development and tests (SPEC §1).

## Consequences

- The launcher registers `gamedist://` on Windows, Linux and macOS.
- `gamedist://` always implies HTTPS; there is no way to downgrade a normal
  link to plain HTTP. `gamedist+http://` links should not be published.
- A rename would touch the protocol string, the well-known path, config
  directories (`~/.config/gamedist`) and the keyring service name.
