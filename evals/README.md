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

```sh
pip install opendatasuite==0.0.2
ONTO_LLM_BASE_URL=<OpenAI-compatible URL ending in /v1> ONTO_LLM_MODEL=<model> ONTO_LLM_API_KEY=<key> \
  cargo run -p onto-testkit --features llm --bin onto-eval -- --runs 3
```

Writes `target/eval/report.md` (pass counts per task, criterion and
condition), `results.json`, and one transcript per run that can be replayed as
a regression test. Or run the `Evaluate` workflow manually with the
`ONTO_LLM_API_KEY` repository secret set.

Four tasks are a starting point, not a benchmark. Add tasks from real
modelling questions as they come up; small differences between conditions
mean little until there are more tasks and runs.
