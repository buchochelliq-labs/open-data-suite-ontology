# How the ontology complements OpenDataSuite

OpenDataSuite (ODS) is the engine that understands an analytics project: it
reads dbt artifacts, derives keys and relationships with evidence, computes
column lineage and impact, and plans builds. The ontology does not repeat any
of that. It adds what ODS has no place for:

- **meaning**: business concepts, definitions, which assets represent which concepts;
- **agreement**: grain, ownership, standards and decisions a team has reviewed;
- **provenance across sources**: ODS, central packs, catalogs and studios side by
  side, with conflicts kept visible;
- **a review path** for agents: propose, then a person accepts.

## Who answers what

| Question | Answered by |
|---|---|
| What models, columns and keys exist? How do we know a key? | ODS (`ods erd`, `ods_describe_entity`); mirrored into the ontology as `observed`/`declared`/`inferred` claims |
| What does a change affect? Where is lineage unknown? | ODS only (`ods_lineage`, `ods_impact`, `ods_list_opaque`) |
| How should I join these tables? | ODS (`ods_plan_query`), checked against the agreed grain in the ontology |
| What is a customer? Which model represents it? What grain did we agree? | Ontology |
| Do our sources disagree? Which one wins, and why? | Ontology |
| Record a decision the user just made | Ontology (`ontology_propose`, then `onto proposals accept`) |

## How they connect

**Shared ids.** An asset is its ODS node id (the dbt `unique_id`, e.g.
`model.jaffle_ods.orders`); a column is `<node id>#<column>`. Concepts use
ontology ids (`concept:order`). Links between them are explicit claims
(`represents`), never name matching.

**Shared evidence vocabulary.** ODS's `basis` becomes authority, and is kept as
a qualifier:

| ODS basis | Ontology authority |
|---|---|
| declared (constraint) | declared |
| tested (data test) | observed |
| joined (SQL joins on it) | observed |
| inferred (naming only) | inferred |

Both follow the same rules: inference is labelled, missing evidence is
uncertainty, a relationship is not lineage.

**ODS → ontology (built).** Source kind `ods` runs
`ods erd generate --all --format json --json` and reads the 1.0 JSON envelope
with ODS's own `ods-erd` types (pinned to the ODS release tag). Nothing in dbt's
artifacts is re-parsed here.

```toml
[[source]]
id = "dbt"
layer = "repository"
kind = "ods"
target_dir = "target"      # optional; ODS's defaults otherwise
infer = false              # include naming-convention keys as `inferred`
```

**Agents use both servers.** `onto mcp` and `ods mcp` run side by side; the
`ontology-grounding` skill routes structure, lineage and impact to ODS and
meaning and agreement to the ontology, passing ids unchanged.

**Ontology → ODS (proposed).** ODS would read the ontology through a
provider behind an SDK contract, so its dashboard, `ods_find_data`,
`ods_describe_entity`, `ods_plan_query` and health checks can show concepts,
agreed grain, owners and standards. See
[the draft ADR](proposals/ods-adr-semantic-context.md). It needs an ODS issue
and ADR before any work in that repository.

**Shared presentation.** Both CLIs render with rs-rich and offer `--json`.

## Other projects

| Project | Role | How it connects |
|---|---|---|
| ontobricks | OWL classes, R2RML mappings, SHACL shapes in Databricks | Planned RDF source over its GitHub mirror (`.owl.ttl`, `.r2rml.ttl`, `shacl.ttl`), as `declared` |
| ontos | Data contracts (ODCS), data products (ODPS), domains, teams | Planned readers for ODCS/ODPS files and its Turtle exports |
| dbt-skills | Earlier Python prototype | Superseded; see [MIGRATION.md](MIGRATION.md) |

RDF export through Oxigraph (one named graph per source) gives these tools a
standard way back in.
