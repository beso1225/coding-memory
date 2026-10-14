---
schema_version: 1
id: queue-leases
title: Recover expired capture leases
summary: Reclaim expired capture work without accepting a stale worker result.
scope:
  kind: project
  project_id: alpha
status: active
conditions:
  - SQLite WAL and a token-fenced capture claim
verification: observed
evidence:
  - bundle: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    item: fixture-E1
review:
  decision: accepted
  backend: fixture
  model: synthetic
  prompt_version: fixture-v1
  reviewed_at: 10
related: []
created_at: 10
updated_at: 10
---

Use `State::claim_store` for queued capture work. Call `sync_all` before publishing
an evidence digest for `src/state.rs`. A stale claim cannot finalize new evidence.
