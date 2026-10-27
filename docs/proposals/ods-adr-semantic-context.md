# Draft ADR for OpenDataSuite: semantic context from the ontology

- **Status:** Draft for discussion; not filed in ODS. Written in ODS's ADR format.
- **Context repository:** open-data-suite-ontology

## Context
ODS knows a project's structure: assets, columns, keys with evidence, lineage,
build state. It has no place for meaning that people agree on: which business
concept a model represents, the grain the team agreed, who owns it, which naming
standard applies. Several ODS features would improve with it: `ods_find_data`
("tables by meaning"), `ods_describe_entity`, `ods_plan_query` (grain-aware join
paths), the `ods serve` catalog, and health checks. The ontology already consumes
ODS (`ods erd generate --json`) and uses ODS node ids as asset ids.

## Options considered
- **Read the ontology's MCP server from ODS.** Loose coupling, but ODS's
  composition root would become an MCP client, and offline reads need a server.
- **A `semantic_context` SDK contract with an `ods-provider-ontology` provider (proposed).**
  Matches ADR-0001/ADR-0006: modules depend on the contract, the CLI wires the
  provider. The provider reads the ontology's local store (`onto-store`) and
  resolution (`onto-core`); no network.
- **Copy concepts into ODS's own store.** Duplicates the ontology and its review flow.

## Decision (proposed)
- New contract `semantic_context` in `ods-sdk`:
  `context(node_id) -> Option<NodeMeaning>` and `search(text) -> Vec<NodeMeaning>`,
  where `NodeMeaning` carries concepts represented, agreed grain, owner, applicable
  standards, and per value its source, authority and any unresolved conflict.
- `ods-provider-ontology` implements it over `onto.toml` / `.onto/store.db`; a
  fake in `ods-provider-fake` and conformance tests per ADR-0006.
- Capability `semantic_context`. Modules degrade conservatively without it.
- Consumers, in order: `ods_describe_entity` and `ods_find_data`, the catalog
  screen, a health check "grain differs from the agreed grain", `ods_plan_query`.
- Ontology relationships are never ERD facts (rule 6). They are shown as
  context, not keys.

## Consequences
- Positive: ODS answers "what does this mean" without owning it; one review flow
  for agreed facts; ids already shared.
- Negative: a cross-repository dependency (`onto-core`, `onto-store`) pinned by
  release; both projects are 0.0.x.
- Dependency direction stays acyclic per crate: the ontology's `onto-ods` depends
  on `ods-erd`; `ods-provider-ontology` depends on `onto-core`/`onto-store`; neither
  core depends on the other.
