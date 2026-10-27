# Architecture

## Decisions so far

| Question | Decision |
|---|---|
| Generic or dbt-specific? | Generic core. dbt structure comes from OpenDataSuite (source kind `ods`); the ontology complements ODS and never re-parses dbt artifacts. See [ECOSYSTEM.md](ECOSYSTEM.md). |
| Where does the central ontology live? | Git packs now, pinned by `onto.lock`. A hosted service later. Consumption goes through the `Source` trait, so neither the store nor agents care where facts come from. |
| Build our own graph, or adopt Graphiti/Cognee? | Build our own core. Graphiti's LLM-driven ingestion is non-deterministic (bad for CI and review), needs a graph-database service and has no notion of authority or review. It can later be a *source* producing `proposed` or `inferred` claims from unstructured material. We took its bi-temporal idea: claim validity (`valid.from/until`) is separate from when we learned it (`retrieved_at`, `source_version`). |
| Storage | SQLite with FTS5 and indexed edges, following dbt-skills ADR 0001. Encryption at rest (SQLCipher through `rusqlite`'s `bundled-sqlcipher`) is a planned feature flag. Graphiti would not help here. |
| UX | Deferred. The CLI and MCP server come first. |
| Python | PyO3 bindings later. Python providers can already plug in today as `command` sources. |

## Model

```
Claim       subject  predicate  object(literal | entity)  qualifiers  valid{from,until}
Provenance  source_id  layer  authority  locator  source_version  retrieved_at  rationale  attributed_to  derived_from
Record      = Claim + Provenance
```

- **Layer** says where a claim lives: `central`, `repository` or `local`.
- **Authority** says how it came to be asserted: `reviewed` (a human accepted
  it), `declared` (a system that owns the fact), `observed` (deterministically
  derived), `inferred` (heuristic or model) or `proposed` (pending, never a value).
- Claim ids are content hashes, so agreement between sources is detected by
  identity, never by matching names.
- Schema is itself claims: `onto:type`, `onto:label`, `onto:definition`,
  `onto:alias`, `onto:cardinality`, `onto:range`, `onto:subtype_of`.

## Resolution

Records for one entity are grouped by predicate. Identical claims merge and keep
every source. On a single-valued predicate, distinct values form a conflict:

- `priority` mode: the configured highest-priority source wins, and the
  conflict and its losing candidates are still returned.
- No listed source, two values from the same source, or `preserve` mode: the
  conflict stays `unresolved` and has no usable value. The agent must ask.
- `proposed` claims appear under `pending` and never as values.
- `as_of` filters by claim validity (defaults to now in the MCP server).

Cardinality comes from `onto:cardinality` claims, then built-ins. Otherwise
literal-valued predicates are single-valued and relations are multi-valued.

## Sources and sync

```rust
trait Source { fn info(&self) -> SourceInfo; fn fetch(&self, cursor: Option<&str>) -> Result<Page, SourceError>; }
```

| Kind | Use |
|---|---|
| `git` | Central packs. The commit is pinned in `onto.lock`; `--locked` refuses unpinned sources. |
| `pack` | A directory of YAML in the repository (`.ontology/`). Committed files default to `reviewed`. |
| `ods` | A dbt project through OpenDataSuite (`ods erd generate --json`), read with ODS's `ods-erd` types. |
| `command` | Any provider: a command printing JSON-lines records. It cannot choose its own `source_id` or layer. |

Sync fetches all pages and then replaces the source's generation atomically. A
failing source keeps its previous generation and is reported (stale-on-error).
A hosted central service would be one more `Source` implementation with cursor
paging.

## Agent boundary

MCP tools are read-only except `ontology_propose`, which stores a pending
proposal in machine-local state (`.onto/`). Acceptance (`onto proposals accept`)
is a human CLI action. It appends a `reviewed` claim to the repository pack,
with reviewer and proposal digest, for the person to commit. Results carry a
notice that text is untrusted evidence. Graph walks are bounded and report
`truncated`.

## Testing agent/human interaction

`onto-testkit` drives the real MCP server in-process over a duplex pipe.

- `Fixture` copies a fixture workspace, turns `packs/` into a local git repo
  as the central source, and syncs.
- `Session` records every tool call, human message and human action in a
  `Transcript` (JSON lines).
- `invariants::check` enforces properties that hold whoever plays the agent: no
  repository change without a human acceptance, acceptance touches only
  `accepted.yaml`, and proposals need a rationale. It warns when an agent
  claims something was saved without an acceptance.
- Tier 1: scripted YAML scenarios. Tier 2: `agent::run_episode` against an
  OpenAI-compatible model (llama.cpp with a small GGUF model in CI, or a hosted
  model). Tier 3: `replay` re-runs recorded transcripts.

Small models are weak at tool use, so tier 2 asserts safety, not design
quality. Its context is the fixture's tool results (bounded, truncated to a
budget) plus the skill text, which fits in an 8–16k window.

## Next

1. Rich CLI on rs-rich; inferred facts only fill gaps; Oxigraph RDF export.
2. Policy and naming (`onto-policy`): standards as entities, template resolution, convention discovery.
3. Catalog providers (Unity Catalog, OpenMetadata). Permission-scoped, on-demand
   catalog reads may need a "live source" path beside sync.
4. SQLCipher feature, optional embeddings (`fastembed` + `sqlite-vec`) if
   lexical search proves insufficient on real queries.
5. Hosted central service as a `Source`; PyO3 bindings; UX.
