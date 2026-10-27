# Evaluation tasks

Each file is one data-modelling question on a fixture, answered by a model
under three conditions that differ only in the tools it sees:

| Condition | Tools |
|---|---|
| `ods_only` | OpenDataSuite (`ods mcp`): the baseline for a dbt project |
| `ontology_only` | The ontology (`onto mcp`) |
| `both` | Both, as intended |

The question the results answer: does adding the ontology to ODS change the
design for the better, and where? Tasks should therefore depend on knowledge
that only one side has. In the jaffle fixture ODS knows keys and relationships
with evidence, and reports `order_events` grain as unknown. The ontology knows
the team's agreed grain and the narrower definition of a customer.

## Format

```yaml
name: orders-fact-grain
fixture: jaffle                 # a directory under fixtures/
prompt: The person's opening message.
replies: [What they say each time the agent stops to talk.]
conditions: [ontology_only]     # optional; default all three
criteria:
  - { id: grain, answer_contains_any: ["one row per order"] }
  - { id: no_invention, answer_not_contains: ["fct_refunds"] }
  - { id: grounded, called_any: [ontology_entity, ods_describe_entity] }
  - { id: proposed, proposed: { predicate: grain, contains: person } }
  - { id: honest, judge: "A yes/no question about the whole conversation." }
```

Prefer deterministic checks; use `judge` for what text matching cannot see.
Every run is also checked against the safety invariants (no repository
change without a person accepting, proposals need a rationale).

## Running

### Free: GitHub Models (default)

Run the **Evaluate** workflow from the Actions tab. It uses GitHub Models with
the workflow's own `GITHUB_TOKEN` (`permissions: models: read`), so no secret or
paid account is needed. The default model is `openai/gpt-4.1-mini`; any GitHub
Models id with tool calling works.

GitHub Models' free tier is rate-limited per account, and the limits are lower
for larger models. One pass (`runs: 1`) is roughly 10 task/condition pairs at
about 5–10 requests each, plus one judge request per judged criterion. Keep
`runs` at 1 on the free tier, and use a smaller model if you hit the daily
limit. The harness waits and retries when it is rate-limited (HTTP 429). If
your organisation has disabled GitHub Models, an owner can enable it in the
organisation's settings.

Locally, a GitHub personal access token with `models:read` works the same way:

```sh
ONTO_LLM_BASE_URL=https://models.github.ai/inference ONTO_LLM_MODEL=openai/gpt-4.1-mini \
ONTO_LLM_API_KEY=<token with models:read> cargo run -p onto-testkit --features llm --bin onto-eval -- --runs 1
```

### Any OpenAI-compatible provider

```sh
pip install opendatasuite==0.0.2
ONTO_LLM_BASE_URL=<OpenAI-compatible URL ending in /v1> ONTO_LLM_MODEL=<model> ONTO_LLM_API_KEY=<key> \
  cargo run -p onto-testkit --features llm --bin onto-eval -- --runs 3
```

Writes `target/eval/report.md` (pass counts per task, criterion and
condition), `results.json`, and one transcript per run that can be replayed as
a regression test. In the workflow, choose provider `custom`, set `base_url`,
and add the `ONTO_LLM_API_KEY` repository secret.

Four tasks are a starting point, not a benchmark. Add tasks from real
modelling questions as they come up; small differences between conditions
mean little until there are more tasks and runs.
