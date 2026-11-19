---
title: Event tracking history in the app database
sources:
  - "Product engineering changelog, tracking v2 release, 2025-03-01"
  - "Data team incident review, 2025-04"
tags: [events, active users, DAU]
confidence: high
verified: 2025-04-10
summary: >-
  Until tracking v2 shipped on 2025-03-01, the app logged every user action as a generic
  page_view, so core actions (project_created, report_viewed, query_run) cannot be told apart
  before March 2025. Any history of active users before then can only count logins or events.
---

# Event tracking history

Tracking v2 shipped on 2025-03-01. Before it, `app_events.event_name` was `page_view` for
almost everything a user did, so the core actions that define an
[[Active user]] only exist from March 2025 onward.

For a longer history, the data team's advice is to show events-based activity before March
2025 as a separate, clearly labelled series rather than mixing it with the agreed definition.
