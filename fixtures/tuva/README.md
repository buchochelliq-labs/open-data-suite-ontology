# Tuva: a real dbt project with a made-up team layer

[The Tuva Project](https://github.com/tuva-health/tuva) (Apache-2.0) is an
open-source healthcare data model used in production: claims and clinical
data normalised into core tables such as `core__medical_claim`,
`core__member_month` and `core__cost`. Its dbt project documents its own
tables thoroughly, so OpenDataSuite alone answers many questions about it.

The team layer (`.ontology/`) is a made-up health plan's analytics team,
recording what a team adopting Tuva has to decide for itself:

- `concepts.yaml`: their PMPM definition (paid, medical plus pharmacy, member
  months as denominator), the three-month claims run-out, counting people
  rather than members, and the decision to exclude a demo data source.
- `assets.yaml`: which Tuva models measure PMPM or are subject to run-out.
- `finance.yaml` and `care-management.yaml`: two definitions of a high-cost
  member that disagree, left unresolved.

The four `tuva-*` tasks in `evals/` ask about these.

## Building the manifest

The manifest is not committed. `build.sh` clones Tuva at a pinned commit,
installs dbt with DuckDB and runs `dbt compile` on Tuva's integration project
(core package only), then copies `manifest.json` into `target/`. Compiling,
rather than only parsing, puts each model's compiled SQL in the manifest, so
ODS can read the joins between Tuva's models instead of treating every model
as opaque. Compile connects to an empty local DuckDB file but runs nothing:
no models, seeds (Tuva's terminology data is not loaded) or hooks. The script
fails if any enabled model lacks compiled SQL.

This runs Tuva's dbt code (its macros and its dbt_utils dependency), so it
runs in its own job: `.github/workflows/tuva-fixture.yml`. That job has no
permissions, no environment and no secrets. It caches the manifest until
`build.sh` changes and uploads it as the `tuva-manifest` artifact. CI and the
evaluation workflow call it and download the manifest into `target/` before
their tests. To build it yourself, do it in a sandbox, not on a machine with
credentials:

```sh
fixtures/tuva/build.sh
```

Without the manifest, the fixture's tests skip and its evaluation tasks are
reported as not run, with the reason.
