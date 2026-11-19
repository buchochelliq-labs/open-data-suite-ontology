#!/usr/bin/env bash
# Fetch the Tuva Project's documentation (Apache-2.0) as this overlay's wiki:
# the markdown pages of its Docusaurus site, at a pinned commit. The upstream
# repository (tuva-health/docs) is no longer public; this is a public fork of it.
# Only markdown is kept (no images or site code). Not committed: run it before
# evaluating with `--overlay tuva-docs-wiki`.
set -euo pipefail

DOCS_REPO=https://github.com/bradmontierth/docs
DOCS_SHA=26af7bd7e5f45ee67cc414724e9d0e538fc9dc8b # 2026-03-11

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git clone --quiet --filter=blob:none "$DOCS_REPO" "$work/docs"
git -C "$work/docs" checkout --quiet "$DOCS_SHA"

rm -rf "$here/tuva-docs"
mkdir -p "$here/tuva-docs"
(cd "$work/docs/docs" && find . -type f \( -name '*.md' -o -name '*.mdx' \) -print0 | xargs -0 cp --parents -t "$here/tuva-docs")
cp "$work/docs/LICENSE.md" "$here/tuva-docs/_LICENSE.md"
printf 'Fetched from %s at %s (Apache-2.0, see _LICENSE.md).\n' "$DOCS_REPO" "$DOCS_SHA" > "$here/tuva-docs/_SOURCE.md"
echo "fetched $(find "$here/tuva-docs" -name '*.md*' -not -name '_*' | wc -l) pages into $here/tuva-docs"
