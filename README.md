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
cargo install onto-cli                        # installs `onto` (from 0.1.1)
cd your-repo
onto init --central https://github.com/buchochelliq-labs/open-data-suite-ontology
echo .onto/ >> .gitignore
onto sync                                     # fetches sources, writes onto.lock
onto search customer
onto show concept:customer                    # resolved values, provenance, conflicts
```

Connect your coding agents in one step:

```sh
onto init --agents claude-code,copilot,generic   # also works after onto init
```

- **Claude Code:** `.mcp.json` with the `onto` server (and `ods` for a dbt
  project), the `ontology-grounding` and `steno` skills in `.claude/skills/`, and
  `CLAUDE.md` importing your digest.
- **Copilot:** `.vscode/mcp.json`, the agent in `.github/agents/`, the skills
  in `.github/skills/`, and a pointer in `.github/copilot-instructions.md`.
- **Generic:** a section in `AGENTS.md` naming the MCP servers and the CLI
  fallback.

Commit those files. Re-running is safe: MCP configs are merged and onto's
Markdown sections are replaced.

For Claude Code there is also a plugin with the same servers, skills and
agent (it needs `onto` and `ods` on PATH):

```
/plugin marketplace add buchochelliq-labs/open-data-suite-ontology
/plugin install onto@open-data-suite-ontology
```

Prebuilt `onto` binaries for Linux, macOS and Windows are attached to each
[release](https://github.com/buchochelliq-labs/open-data-suite-ontology/releases), and
the crates are on crates.io (`onto-cli` and its libraries `onto-core`, `onto-store`,
`onto-ods`, `onto-workspace`, `onto-mcp`). From a checkout, `cargo install --path
crates/onto-cli` installs `onto` too.
Releases follow the buchochelliq-labs standard,
[release-kit](https://github.com/buchochelliq-labs/release-kit): `release-kit.yml`
says what is published and built, and `release.py tag` on `main` starts the release.

Agents can read (`ontology_status`, `ontology_overview`, `ontology_search`,
`ontology_entity`, `ontology_neighbors`, `ontology_context`) and propose
(`ontology_propose`). `ontology_overview` shows what the graph contains (types,
predicates, main concepts, standards, sources) for when a search finds nothing.
They cannot sync, accept or write files. Agents only learn from tools they
think to call, so they also get a digest of the team's agreed facts to read up
front: `onto init` sets `[context] path = ".onto/context.md"` in `onto.toml`,
and every `onto sync` rewrites it (`onto context --out <file>` writes one by
hand).

The digest is generated per person from their synced store (like the store,
don't commit it unless your team chooses to), stays within a size budget
(`--max-chars`), and cites each fact's source and authority. A person reviews proposals:

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
| `skills/`, `agents/`, `.github/agents/` | Agent skills and the agent definition (Claude Code and Copilot formats), tested against the live tool list |
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

Separately, [`evals/`](evals/README.md) measures whether the ontology helps: the
same modelling tasks run with OpenDataSuite's tools only, the ontology's only,
and both, scored by checks and a judge model (`onto-eval`, or the manual
`Evaluate` workflow, which has free provider presets).

## Status

First slice. See [docs/MIGRATION.md](docs/MIGRATION.md) for what has been ported
from dbt-skills and what has not.

## License

Apache License 2.0: see [LICENSE](LICENSE). OpenDataSuite, which `onto` builds on,
is also Apache-2.0.
