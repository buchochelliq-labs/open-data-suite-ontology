# Evaluation tasks

Each file is one data-modelling question on a fixture, answered by a model
under conditions that differ in the tools it sees and the context it is given:

| Condition | Tools |
|---|---|
| `ods_only` | OpenDataSuite (`ods mcp`): the baseline for a dbt project |
| `ontology_only` | The ontology (`onto mcp`) |
| `both` | Both, as intended |

Add `+digest` to any of them (`both+digest`, `ods_only+digest`) to put the
context digest from `onto context` in the system prompt: the team's agreed
facts, always in front of the agent, without it having to think to ask.
Comparing `both` with `both+digest` and `ods_only+digest` shows what tools add
over a digest and the other way round.

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
persona:                        # for runs where a model plays the person
  role: analytics engineer asked by finance for an orders fact table
  goal: the right grain and key, matching what the team agreed
  knows: [Facts they can share when asked.]
  accepts_proposals: false      # whether they accept a proposal they agree with
conditions: [ontology_only]     # optional tool sets; default all three
criteria:
  - { id: grain, answer_contains_any: ["one row per order"] }
  - { id: no_invention, answer_not_contains: ["fct_refunds"] }
  - { id: grounded, called_any: [ontology_entity, ods_describe_entity] }
  - { id: proposed, proposed: { predicate: grain, contains: person } }
  - { id: honest, judge: "A yes/no question about the whole conversation." }
```

Prefer deterministic checks; use `judge` for what text matching cannot see.

### Tasks that need exploration

Besides looking up a named model, some tasks only pass if the agent explores:

| Task | What it needs |
|---|---|
| `revenue-measure-multi-hop` | Two hops: `orders` → `represents` → `concept:order` → `measured_by` → `concept:net_sales` |
| `revenue-definition-paraphrase` | The person says "revenue"; the agreed concept is "Net sales", and the word "revenue" appears nowhere |
| `order-history-unnamed` | The person says "order history"; the model is `order_events` |
| `naming-standard` | A central standard the question doesn't mention, which the project's own model names don't follow |

The report adds an exploration table per condition (tool calls, entities
looked up, characters of tool output read) and lists every search that found
nothing, so vocabulary misses are visible. They decide whether semantic search
(#12) moves up.

### Who plays the person

By default the person follows the task's `replies`. With `--human simulated`
(workflow input `human`), a second model plays the person from the task's
`persona`: it sees the conversation as a person would (messages, not tool
calls), shares what it knows when asked, ends the conversation when it has
what it needs (`DONE`), and accepts a proposal when its persona allows and it
agrees (`ACCEPT`). One model plays the agent, another the person. The person's
model is `ONTO_HUMAN_BASE_URL` / `ONTO_HUMAN_MODEL` / `ONTO_HUMAN_API_KEY`
(`ONTO_HUMAN_MODEL` for Claude Code), defaulting to the agent's. A simulated
person costs one request per reply.
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
| `local` | Free, no account | None | llama.cpp on the runner. `model` is a Hugging Face `repo/file.gguf` (default Qwen2.5 7B). Slow on CPU, and a weaker agent and judge |
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

## Spending fewer tokens

The report has a token table per condition: the agent's requests, new and
cached input and output, and the person's and judge's totals, as the models
reported them. `onto-eval` prints the run's total at the end. To keep runs
cheap:

- **Replay instead of rerunning.** Conversations recorded in
  `evals/recorded/` replay in CI on every change, at no token cost. A failure
  means the ontology now answers those calls differently: check the change,
  then re-record with a live run.
- **Run live only when tasks, tools or skills change**, with `runs: 1` and the
  comparison that matters (e.g. `conditions: ods_only,both`, or `task` for one
  task).
- **Helpers are cheap.** The judge answers all of a conversation's questions
  in one request. For Claude Code the judge and the simulated person replace
  Claude Code's own system prompt instead of loading it, and the workflow runs
  them on Haiku (`ONTO_JUDGE_MODEL`, `ONTO_HUMAN_MODEL`). `judge: false`
  skips them entirely.
- **Use the free `local` provider for routine runs**, and Claude for a final check.

Agents themselves spend fewer tokens because the ontology's answers are
compact (one line per fact; `detail: full` for provenance), leave facts only
dbt asserts to ODS (`include: all` to get them), and `ontology_context`
answers in one call what used to take a search, an entity and its
neighbours. The `steno` skill, given in every condition, keeps working notes
brief and the final answer complete.

Eight tasks are a starting point, not a benchmark. Add tasks from real
modelling questions as they come up; small differences between conditions
mean little until there are more tasks and runs.
