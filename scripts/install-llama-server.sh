#!/usr/bin/env bash
# Download llama.cpp's prebuilt Ubuntu x64 CPU build into DIR and print the
# path of its llama-server. Releases are frequent and the newest one can lack
# the asset for a while (or its name can change), so this tries the newest
# releases in turn and, if none has it, lists what the newest one offers.
# Needs gh with GH_TOKEN.
set -euo pipefail
dir=${1:?usage: install-llama-server.sh DIR}
pattern='*-bin-ubuntu-x64.tar.gz'
repo=ggml-org/llama.cpp
mkdir -p "$dir"
tags=$(gh release list --repo "$repo" --limit 10 --exclude-drafts --exclude-pre-releases --json tagName --jq '.[].tagName')
for tag in $tags; do
  into="$dir/$tag"
  mkdir -p "$into"
  if gh release download "$tag" --repo "$repo" --pattern "$pattern" --dir "$into" --clobber 2>/dev/null; then
    for archive in "$into"/*.tar.gz; do tar -xzf "$archive" -C "$into"; done
    server=$(find "$into" -name llama-server -type f | head -1)
    if [ -n "$server" ]; then
      echo "llama.cpp $tag" >&2
      echo "$server"
      exit 0
    fi
  fi
done
newest=$(echo "$tags" | head -1)
echo "No llama.cpp release among the newest 10 has $pattern with a llama-server. Assets of $newest:" >&2
gh release view "$newest" --repo "$repo" --json assets --jq '.assets[].name' >&2
exit 1
