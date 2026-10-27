# Migration from dbt-skills

Status of each dbt-skills area in this repository. "Ported" means the behaviour
exists here with tests, not that every option carried over.

| dbt-skills | Here | Status |
|---|---|---|
| `ontology.py` claim model, `ClaimRecord`, provider protocol | `onto-core` `Claim`, `Record`, `Source` | Ported. Adds layers, authority tiers, validity and content-hash ids. |
| `ontology.py` local store, `ontology_sources.py`, `ontology_cli.py` | `onto-store`, `onto-workspace`, `onto` CLI | Ported: generations, search, neighbours, sync status. Not ported: `SyncScope` include/exclude filters, pause/disable, purge, incremental sync. |
| ADR 0001 (SQLCipher, FTS5 baseline) | `onto-store` | SQLite + FTS5 (Porter stemming) done. SQLCipher not yet. |
| `catalog.py` ordered resolution, conflicts | `onto-core::resolve` | Ported: priority/preserve modes, conflicts with candidates, no name-based merging. |
| `providers.py`, `unity_catalog.py`, `openmetadata.py`, `catalog_cache*.py` | `Source` implementations | Not started. Usable today through `command` sources. Permission-scoped caching needs a design for on-demand reads. |
| `business_context*.py`, `local_context_updates.py`, `context_review.py` | Pack YAML + `proposals` | Partly ported: propose, digest, human accept, read back. Not ported: rebase previews and same-record conflict review. |
| `modelling_policy.py`, `modelling_naming.py`, `convention_discovery.py`, `policy_comparison.py`, `decision_proposals.py`, `design_decision*.py` | `onto-policy` (planned) | Not started. Standards are already representable as `ModellingStandard` entities. |
| `project.py`, `project_artifacts.py`, `project_catalog.py` | dbt source (planned) | Not started. |
| `model_planning.py`, `planning_contracts.py` | dbt domain crate (planned) | Not started. |
| `modelling_mcp.py`, `uol_mcp.py` | `onto-mcp` | 5 generic tools. dbt-specific tools return with the dbt crate. |
| `ods_mcp.py` | `command` or native source | Not started. |
| `.github/skills/dbt-context-routing`, `.github/agents/*` | `skills/ontology-grounding`, `agents/ontology-modelling` | Rewritten for the generic tools and tested against them. |
| `cache_security.py`, `workspace_store.py`, `application_*`, `native_*`, `frontend/` | — | Deferred (encryption, UX). |
| `experiments/offline_backends` | — | Findings kept as context in ARCHITECTURE.md; code not ported. |
