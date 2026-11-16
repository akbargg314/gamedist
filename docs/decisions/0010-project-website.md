# 0010. Static project website on GitHub Pages

Status: Accepted (supersedes [0004](0004-no-standalone-website.md))

## Context

ADR 0004 decided against a standalone website (`apps/website`, a SvelteKit
developer portal or public aggregator) because the server's own portal covers
game pages, the developer dashboard and administration.

That still holds for games, but the project itself has no public page. The
README is written for contributors, and a server's portal describes that
server's games, not GameDist. People who have not cloned the repository need
a place that explains what GameDist is, offers downloads once releases exist,
and publishes the mdBook documentation, including the guide to running a
server.

## Decision

The project has a small static website in `site/`: a landing page, a
downloads page and the mdBook from `docs/`, published under `docs/` of the
site. It is hand-written HTML and CSS with a little vanilla JavaScript, with
no framework and no build step of its own, no trackers, no third-party
scripts or fonts, and no cookies. The downloads page asks the GitHub Releases
API for the latest release from the visitor's browser and falls back to
build-from-source instructions when there is none.

`site/build.sh` assembles the site and the book; `.github/workflows/pages.yml`
runs it and deploys to GitHub Pages at <https://akbargg314.github.io/gamedist/>.

Game catalogs stay on the servers. The site lists no games, hosts no builds,
and is not a registry or an aggregator.

## Consequences

- Servers are unchanged: they still ship only the server-rendered portal, one
  binary, no Node.js. A cross-server aggregator is still not provided.
- The downloads page needs no edits when releases appear; it shows whatever
  the latest GitHub release contains. Visitors' browsers contact
  `api.github.com` for that list (unauthenticated, rate-limited per IP).
- The landing page's status section repeats the README status table in short
  form and must be updated with it.
- Links in the book that pointed outside it (`../../protocol/…`) now point at
  the repository on GitHub, so they work in the published book.
- `docs/book.toml` sets `site-url = "/gamedist/docs/"`, and `site/404.html`
  and the Open Graph tags use absolute URLs; these change if the site moves to
  a custom domain. All other links are relative.
- GitHub Pages must be enabled with "GitHub Actions" as the source.
