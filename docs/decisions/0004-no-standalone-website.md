# 0004. No standalone website for v1

Status: Superseded by [0010](0010-project-website.md)

## Context

PLAN §9.4: is a separate `apps/website` (SvelteKit dev portal / public
aggregator) needed, or does the server's own portal suffice?

## Decision

v1 ships **only the server-rendered portal** inside `gamedist-server`
(templates with `maud`): landing page, public game pages, developer dashboard
(games, releases, channels, API tokens, keys) and admin pages (users,
settings, invites, takedowns, audit log). There is no `apps/website`.

## Consequences

- One binary to deploy; no Node.js on servers; no separate API auth story for
  a web frontend.
- Pages work without JavaScript.
- A public cross-server aggregator is not provided; servers expose
  `/api/v0/export.jsonl` so one can be built later as a separate project.
