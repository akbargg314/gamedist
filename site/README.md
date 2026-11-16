# Project website

The static site published at <https://akbargg314.github.io/gamedist/>:
landing page (`index.html`), downloads (`download.html`, with `download.js`)
and the mdBook from `docs/`, published under `docs/`. Hand-written HTML and
CSS, no framework and no build step for the site itself. Game catalogs are
not on this site; they live on the servers (see
[ADR 0010](../docs/decisions/0010-project-website.md)).

## Rules

- Every internal link and asset is relative (`style.css`, `docs/launcher.html`),
  so the site works under `/gamedist/` and at the root of a custom domain.
  The exceptions are `404.html` (served for missing paths at any depth, so it
  uses absolute URLs and inline CSS) and the absolute `og:url` / `og:image`
  tags; change those if a custom domain is added.
- No third-party scripts, fonts, trackers or cookies. The only outside request
  is the downloads page asking `api.github.com` for the release list, from the
  visitor's browser. The Content-Security-Policy meta tags enforce this.
- Pages must work without JavaScript; the release list then shows a link to
  GitHub instead.
- Claims must match the status table in the root `README.md`. When that table
  changes, update the Status section of `index.html`.

## Build and preview

Needs [mdBook](https://rust-lang.github.io/mdBook/) 0.5.4 (the version pinned
in `.github/workflows/pages.yml`):

```sh
cargo install mdbook --version 0.5.4 --locked
site/build.sh                    # assembles _site/ (ignored by git)
```

Serve it under the same `/gamedist/` prefix as GitHub Pages:

```sh
mkdir -p /tmp/gd-preview && ln -sfn "$PWD/_site" /tmp/gd-preview/gamedist
python3 -m http.server 8000 --bind 127.0.0.1 -d /tmp/gd-preview
# open http://127.0.0.1:8000/gamedist/
```

`python3 -m http.server -d _site` also works, since links are relative.

## Images

`favicon.svg` is the logo mark. `favicon-32.png`, `apple-touch-icon.png` and
`og-image.png` are generated from the same geometry with
`python3 site/tools/gen-images.py` (standard library only). Neither
`tools/`, this README nor `build.sh` is published.

## Deployment

`.github/workflows/pages.yml` runs `site/build.sh` and deploys the result
with the official GitHub Pages actions on every push to `main` that touches
`site/`, `docs/` or the workflow, and on manual dispatch. In the repository
settings, Pages → Build and deployment → Source must be set to
"GitHub Actions".
