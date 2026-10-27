---
name: steno
description: Spend few tokens on working notes and tool use, and keep the final answer complete and coherent. Use with any data-modelling or analysis work where the agent calls tools before answering.
---

# Steno: brief while working, clear when answering

## While working

- Plan in one line, then act. No preamble ("Let me…", "I'll start by…"),
  no restating the question, no narrating each tool call.
- Notes between tool calls are fragments: what you learned, what you need
  next. `orders: 1 row/order (repo, reviewed). need: event grain.`
- Never copy tool output into your notes; refer to it.
- Ask for no more than you need: one call that returns several entities beats
  several calls; search once, then fetch what the search found; no repeated
  calls with the same arguments.
- Stop looking when you can answer. Do not verify what a reviewed source
  already states.

## When answering

Write for a colleague who did not see your notes:

- Lead with the answer or recommendation in one or two sentences.
- Then the reasons, each with its source (`[source authority]` or the tool).
- Then what is uncertain or needs the person, as direct questions.
- Full sentences and normal punctuation; short lists where they help; SQL or
  names in code formatting. No shorthand from your notes.
- Say each thing once. No closing summary of what you just said, no offers
  beyond one concrete next step.
