# open-data-suite-ontology

A layered, provenance-preserving ontology that agents use to understand what
to build. It merges a **central** ontology (shared vocabulary and concepts,
consumed from git today and from a hosted service later) with **repository**
facts and a **local** overlay. Then it serves the result to agents over MCP,
with every value traceable to its source and every disagreement kept visible.

It complements [OpenDataSuite](https://github.com/buchochelliq-labs/open-data-suite):
ODS understands the dbt project (assets, keys with evidence, lineage, impact);
the ontology adds meaning, agreed facts and a review path, using the same ids.
See [docs/ECOSYSTEM.md](docs/ECOSYSTEM.md). It started as a Rust port of the
context layer in [`dbt-skills`](https://github.com/buchochelliq-labs/dbt-skills);
see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and
[docs/MIGRATION.md](docs/MIGRATION.md).

## Quick start

```sh
cargo install --path crates/onto-cli          # installs `onto`
cd your-repo
onto init --central https://github.com/buchochelliq-labs/open-data-suite-ontology
echo .onto/ >> .gitignore
onto sync                                     # fetches sources, writes onto.lock
onto search customer
onto show concept:customer                    # resolved values, provenance, conflicts
```

Connect an agent (VS Code / Copilot, Claude Code, any MCP host):

```json
{ "servers": { "onto": { "command": "onto", "args": ["--root", "${workspaceFolder}", "mcp"] } } }
```

Agents can read (`ontology_status`, `ontology_search`, `ontology_entity`,
`ontology_neighbors`) and propose (`ontology_propose`). They cannot sync,
accept or write files. A person reviews proposals:

```sh
onto proposals list
onto proposals accept <digest>   # appends to .ontology/accepted.yaml; you commit it
```

In CI, `onto sync --locked` uses the commits pinned in `onto.lock`.

For a dbt project, add OpenDataSuite as a source (`pip install opendatasuite`)
and run `ods mcp` next to `onto mcp`:

```toml
[[source]]
id = "dbt"
layer = "repository"
kind = "ods"
```

## Layout

| Path | What |
|---|---|
| `crates/onto-core` | Claims, provenance, authority, layers, the `Source` trait, conflict resolution |
| `crates/onto-store` | SQLite store: generations per source, FTS5 search, bounded traversal, proposals |
| `crates/onto-workspace` | `onto.toml`, pack format, git/pack/command sources, `onto.lock`, sync, proposal review |
| `crates/onto-ods` | OpenDataSuite as a source: dbt assets, columns, keys and relationships with ODS's evidence |
| `crates/onto-mcp` | MCP server (`rmcp`, stdio) |
| `crates/onto-cli` | The `onto` binary |
| `crates/onto-testkit` | Fixtures, scripted scenarios, replay, model-driven episodes |
| `packs/core` | The central pack: data-modelling vocabulary and example concepts |
| `skills/`, `agents/` | Agent skill and agent definition, tested against the live tool list |
| `scenarios/` | Scripted agent/human scenarios run on every PR |
| `fixtures/` | Fixture workspaces |

## Tests

```sh
cargo test --workspace --all-features
```

Three tiers share one session and transcript format:

1. **Scripted scenarios** (`scenarios/*.yaml`): agent and human steps against
   the real MCP server, with expectations and invariants, on every PR.
2. **Small local model** (`.github/workflows/agent-eval.yml`): llama.cpp
   serves a ~1.5B GGUF model on the CI runner; the model plays the agent and a
   script plays the person. Hard assertions are safety invariants. To run it
   locally, point `ONTO_LLM_BASE_URL` at any OpenAI-compatible endpoint and run
   `cargo test -p onto-testkit --features llm --test llm_episode -- --nocapture`.
3. **Record and replay**: transcripts from model runs are JSON lines that
   replay deterministically as regression tests.

## Status

First slice. See [docs/MIGRATION.md](docs/MIGRATION.md) for what has been ported
from dbt-skills and what has not. No license has been chosen yet.
