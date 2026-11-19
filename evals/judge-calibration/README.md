# Judge calibration

How often does the judge model agree with a person? Most eval criteria are
yes/no questions for a judge model, so every score rests on its verdicts
([#57](https://github.com/buchochelliq-labs/open-data-suite-ontology/issues/57)).

| File | Holds |
|---|---|
| `conversations/*.jsonl` | 40 conversations from eval runs 11–19, as the runner wrote them |
| `labels.yaml` | each judged criterion of each conversation (67 items), with the question as it was asked, and a person's label: `human: pass`, `fail` or `unsure`, and an optional `note` |
| `recorded.json` | the judge's verdict and reason on the same items, from those runs |

Labels and verdicts are kept apart so that labelling does not show what the
judge said.

## Labelling

```sh
cargo run -p onto-testkit --features llm --bin onto-eval -- --label
```

It shows each conversation as the judge saw it (tool results shortened),
then asks each of its questions: `p`ass, `f`ail, `u`nsure, `s`kip or `q`uit.
Every answer is saved at once, so labelling can stop and resume. Editing
`labels.yaml` by hand works too. Judge the substance against the question's
own wording, as the judge is told to; add a `note` when the call is close.

## Checking the judge

```sh
cargo run -p onto-testkit --features llm --bin onto-eval -- --calibrate
```

This reports Cohen's κ (with an approximate 95% interval), the confusion
counts, and every disagreement with the judge's reason, from `recorded.json`
with no model calls. The test `the_judge_agrees_with_the_hand_labels` runs the
same check in CI: it fails when κ is below 0.70 over at least 20 labelled
items. With fewer labels it is reported but not enforced.

`--calibrate --live` asks the judge model again (configured as for an eval
run: `ONTO_AGENT=claude-code` with `ONTO_JUDGE_MODEL`, or `ONTO_JUDGE_*`), one
request per conversation. Add `--record` to save its verdicts as the new
`recorded.json`, e.g. after changing the judge's prompt or model, so CI checks
the judge as it now is.

## How the set was drawn

From the 357 judged verdicts in the artifacts of runs 11–19 (12 tasks), per
task up to two conversations where the judge failed a criterion and up to two
where it passed them all, one per condition where possible, then more
conversations with a fail up to 40. The judge passes about three quarters of
criteria overall, and κ is unstable when nearly every item is a pass, so
fails are over-represented here: 48 recorded passes, 19 fails. The task
questions have not changed since those runs, so the recorded verdicts answer
the questions in `labels.yaml`.

New criteria or a changed question make an item stale: re-record with
`--calibrate --live --record` after relabelling any item whose question
changed.
