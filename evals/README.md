# Evaluation tasks

Each file is one data-modelling question on a fixture, answered by a model
under three conditions that differ in the tools it sees:

| Condition | Tools |
|---|---|
| `ods_only` | OpenDataSuite (`ods mcp`): the baseline for a dbt project |
| `ontology_only` | The ontology (`onto mcp`) |
| `both` | Both, as intended |

Where the ontology's tools are visible, the agent also gets the
`ontology-grounding` skill in its system prompt, as an agent host would load
it. The skill ships with the ontology, so it is part of what is evaluated.
`ods_only` gets equivalent guidance for ODS alone (`ODS_GUIDANCE` in
`eval.rs`): look up every named model, read and cite its grain and keys, say
what is unknown. A difference between conditions then comes from what the
ontology knows, not from extra instructions.

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

Any OpenAI-compatible endpoint with tool calling works. Run the **Evaluate**
workflow from the Actions tab and pick a provider preset:

| Preset | Cost | Setup | Notes |
|---|---|---|---|
| `claude-code` (default) | Your Claude subscription | Run `claude setup-token` locally and add the token as secret `CLAUDE_CODE_OAUTH_TOKEN` | Claude Code itself is the agent, connected to `onto mcp` (and `ods mcp`) as a real MCP host; judged criteria use Claude Code too. `model` takes e.g. `sonnet` or `opus`. Counts against your plan's usage limits; for your own repository only, not shared tokens |
| `gemini` | Free tier | Repository secret `GEMINI_API_KEY` from Google AI Studio, no card | Most free headroom; free-tier inputs may be used by Google for training (the fixtures are synthetic). Model `gemini-flash-latest` |
| `groq` | Free tier | Secret `GROQ_API_KEY` | Low tokens per minute and per day; may not finish a full pass |
| `openrouter` | Free models | Secret `OPENROUTER_API_KEY` | About 50 requests a day on `:free` models |
| `local` | Free, no account | None | llama.cpp on the runner (default Qwen2.5 7B). Slow on CPU, and a weaker agent and judge |
| `custom` | Your provider | Input `base_url`, secret `ONTO_LLM_API_KEY` | Any OpenAI-compatible endpoint |

Keys are secrets of the repository's `ai` environment (**Settings →
Environments → ai**); repository secrets with the same names also work. If the
environment requires reviewers, each run waits for approval before it starts.

Free tiers and model lists change often (GitHub Models, once an option, was
retired on 30 July 2026), so check the provider's current limits. One pass
(`runs: 1`) is roughly 10 task/condition pairs at about 5–10 requests each,
plus one judge request per judged criterion. The harness waits and retries
when it is rate-limited (HTTP 429), but it is better not to hit the limit:
requests are paced to the `rpm` input (`ONTO_LLM_RPM` locally), which defaults to
5 for `gemini`, 30 for `groq` and 20 for `openrouter`. At 5 a minute a full pass
takes roughly 15–20 minutes. Keep `runs` at 1 on free tiers.

### Small daily quotas

Some free tiers allow very few requests a day (Gemini's free tier can be as low
as 20 per model). A full pass will not fit, so run a slice and build up results
over several days:

| Input | Effect |
|---|---|
| `task` | Only tasks whose name contains this, e.g. `order-events-fan-out` |
| `conditions` | e.g. `ods_only,both`, the comparison that matters most |
| `judge` | Off saves one request per judged criterion; deterministic checks still score |
| `max_requests` | Hard cap for the run |

One task under two conditions without the judge is about 10–12 requests. When
the provider reports that its daily quota is used, the run stops at once
(no retries), keeps the completed conversations, and lists the rest as not
run.

Locally:

```sh
pip install opendatasuite==0.0.2
ONTO_LLM_BASE_URL=https://generativelanguage.googleapis.com/v1beta/openai ONTO_LLM_MODEL=gemini-flash-latest \
ONTO_LLM_API_KEY=<key> cargo run -p onto-testkit --features llm --bin onto-eval -- --runs 1
```

While it runs, the job output shows the conversation live: each message the
person sends, each model request and how long it took, each tool call with its
arguments and (shortened) result, and each criterion's score. The same log is
saved to `target/eval/live.log`, which is uploaded even when a run is
cancelled. A tool call that takes more than 60 s, or a conversation that takes
more than 15 minutes, is recorded as an error instead of stalling the run.

Free tiers are sometimes overloaded and answer `503 Service Unavailable`, or
not at all. In the workflow, hosted requests time out after 2 minutes
(`ONTO_LLM_TIMEOUT_SECS`) and, like 503s, are retried with backoff; each retry
appears in the log with the provider's message. If a model stays overloaded,
try another model, e.g. `gemini-flash-lite-latest`.

After a run, its **Summary** page on GitHub shows the score table and every
conversation in full: the person's and agent's messages, each tool call with
its arguments, and each tool result (folded). The same chats are in
`transcripts/*.md` in the artifact.

Writes `target/eval/report.md` (pass counts per task, criterion and
condition), `results.json`, and one transcript per run that can be replayed as
a regression test.

Four tasks are a starting point, not a benchmark. Add tasks from real
modelling questions as they come up; small differences between conditions
mean little until there are more tasks and runs.
