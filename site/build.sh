#!/bin/sh
# Assemble the project website: the static files in site/ plus the mdBook
# from docs/, built into <out>/docs/. Used by .github/workflows/pages.yml and
# for local previews.
#
#   site/build.sh [OUT_DIR]     default OUT_DIR: _site at the repository root
#
# Needs mdbook on PATH, or MDBOOK=/path/to/mdbook.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/_site"}
mdbook=${MDBOOK:-mdbook}

case "$out" in
  /*) ;;
  *) out="$(pwd)/$out" ;;
esac
out=${out%/}

# Refuse to wipe anything that is obviously not a build directory.
case "$out" in
  "" | / | "$HOME" | "$root" | "$root/site" | "$root/docs" | "$root/site/"* | "$root/docs/"*)
    echo "build.sh: refusing to use '$out' as the output directory" >&2
    exit 2
    ;;
esac

if ! command -v "$mdbook" >/dev/null 2>&1; then
  echo "build.sh: mdbook not found; install it (cargo install mdbook --version 0.5.4 --locked) or set MDBOOK" >&2
  exit 1
fi

rm -rf "$out"
mkdir -p "$out"

# Static files, without the files that only document or regenerate the site.
(
  cd "$root/site"
  find . -type f ! -name README.md ! -name build.sh ! -path './tools/*' | while read -r f; do
    mkdir -p "$out/$(dirname "$f")"
    cp "$f" "$out/$f"
  done
)

# The book. -d is resolved against the book root, so pass an absolute path.
"$mdbook" build "$root/docs" -d "$out/docs"

echo "Site assembled in $out"
