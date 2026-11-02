#!/usr/bin/env bash
# Build the dbt manifest of the Tuva Project (https://github.com/tuva-health/tuva,
# Apache-2.0) for this fixture. This runs Tuva's dbt code (macros and packages),
# so run it in CI or another sandbox, not on a machine with secrets.
#
# The manifest is not committed: CI builds it before the tests, and tests and
# evaluation tasks on this fixture are skipped where it is missing.
set -euo pipefail

TUVA_REPO=https://github.com/tuva-health/tuva
TUVA_SHA=9ab0d1c3e4a920f5acebdd5f9a2f456fb20fe7e2 # 2026-10-23
DBT_DUCKDB='dbt-duckdb==1.11.1'

here=$(cd "$(dirname "$0")" && pwd)
build=${TUVA_BUILD_DIR:-$(mktemp -d)}
mkdir -p "$build"

if [ ! -d "$build/tuva/.git" ]; then
  git clone --quiet --filter=blob:none "$TUVA_REPO" "$build/tuva"
fi
git -C "$build/tuva" checkout --quiet "$TUVA_SHA"

if [ ! -x "$build/venv/bin/dbt" ]; then
  python3 -m venv "$build/venv"
  "$build/venv/bin/pip" install --quiet "$DBT_DUCKDB"
fi

# Tuva's integration project with the core package only: the optional
# packages (quality measures, HCCs and so on) are not needed for the fixture.
cd "$build/tuva/integration_tests"
printf 'packages:\n  - local: ../\n' > packages.yml
export DBT_PROFILES_DIR="$PWD/profiles/duckdb" TUVA_DUCKDB_PATH="$build/tuva.duckdb" DBT_SEND_ANONYMOUS_USAGE_STATS=false
"$build/venv/bin/dbt" deps --quiet
# `dbt compile`, not `dbt parse`, so the manifest carries each model's compiled
# SQL: without it ODS treats every model as opaque and finds no joins. Compile
# renders the models' Jinja against an empty local DuckDB file; it runs no
# model, seed or hook, so no data (nor Tuva's terminology seeds) is loaded.
# Introspective macros see missing relations (dbt_utils.star renders `*`).
rm -f target/manifest.json
"$build/venv/bin/dbt" compile

# Every enabled model must have compiled SQL, or the fixture is not what the
# tests expect.
"$build/venv/bin/python" -I - target/manifest.json <<'PY'
import json, sys
nodes = json.load(open(sys.argv[1]))["nodes"].values()
models = [n for n in nodes if n["resource_type"] == "model"]
missing = sorted(n["unique_id"] for n in models if not n.get("compiled_code"))
print(f"{len(models) - len(missing)} of {len(models)} models have compiled SQL")
if not models or missing:
    sys.exit("models without compiled SQL: " + ", ".join(missing[:20]))
PY

mkdir -p "$here/target"
cp target/manifest.json "$here/target/manifest.json"
echo "wrote $here/target/manifest.json ($(du -h "$here/target/manifest.json" | cut -f1))"
