# Initial cross-session quality evaluation

Issue #7 was evaluated on 2026-10-14 with Codex CLI 0.162.1 and `gpt-6.1-sol`.
[The sanitized observed report](../experiments/worker/quality-observed.json) records
the results. This is a small functional demonstration, not a statistical quality
estimate or a guarantee for unrestricted personal histories.

## Method

Three synthetic development runs used the actual CLI persistence and capture
route with controlled loopback source answers. Extraction, consolidation and
independent review used the real authenticated model, fresh contexts and the
generation deny hook. The user independently logged into an evaluation-only
Engine home; normal credentials and personal histories were not copied or read.

A separate real-model coding context received buggy SQLite queue code and MCP
access scoped explicitly to the source project. The consumer directory was a
synthetic code copy, not an automatically inferred Git project. Its instructions
required searching and reading applicable memory before editing. The prompt did
not disclose the non-obvious token increment of seven. Hidden checks verified
increments of 7/14, stale acknowledgment rejection, successful current-token
acknowledgment and repeated acknowledgment rejection. The resulting code also
passed those checks using file-backed SQLite in WAL mode. Consumer tools were
enabled only for this synthetic coding context; memory-processing calls kept
their deny hook.

## Results and limitations

| Case | Observed result |
| --- | --- |
| Explicit project constraint | Two ready candidates became two independently accepted notes; the exact token increment and SQLite WAL applicability were retained |
| Progress only | Successful empty extraction, zero candidates |
| Unsupported speculation | Successful empty extraction, zero candidates |
| Separate-session reuse | Five MCP search calls, two note reads, correct implementation passing protocol checks |

Search required retries: two literal AND queries returned no hits and two requests
failed because the agent requested 20 results against a runtime maximum of 10.
The subsequent `lease` query returned both notes. The MCP schema now advertises
`limit` minimum 1 and maximum 10, with a focused regression check. The real-model
experiment was not rerun to claim that this change eliminated the observed
mistakes. FTS query formation and excessive query terms remain practical weaknesses.

The evaluated support was explicit `user_stated` knowledge. Separate semantic
review accepted both operations without raising their verification to `observed`.
JSON validity and review approval alone were not the reuse criterion: independent
behavior checks were required. Nevertheless, these are three easy synthetic
cases. Tool-observed support, ambiguous contradictions, broader codebases and
Japanese queries require a larger corpus.

No generated semantic Wikilinks were exercised by the real-model corpus. Existing
deterministic consolidation fixtures check allowed existing/accepted targets,
dependencies and rejected links; semantic relationship usefulness remains
unmeasured. Expand the corpus before treating link quality as established.

## Opt-in reproduction

Use a new private evaluation state outside repositories, run `generation-setup`,
then independently log into that home's file credential store as described in
[extraction](extraction.md). The probe requires explicit external-send consent and
may consume model quota. It creates synthetic source histories, a Vault and code
fixtures, and grants external policy only to its synthetic project. Raw private
logs and credentials must not be committed.

```sh
nix develop -c python3 experiments/worker/quality_probe.py \
  --engine target/debug/coding-memory --codex /absolute/path/to/codex \
  --state-dir /absolute/path/to/private-evaluation-state \
  --model gpt-6.1-sol --allow-external
```

Use a fresh state for each run; preserve only sanitized evaluation results. The
temporary authenticated home from the observed run is not a production setup.
Ordinary CI remains offline. [The worker probe](worker.md) validates continuous
operation and launchd separately using synthetic model responses.
