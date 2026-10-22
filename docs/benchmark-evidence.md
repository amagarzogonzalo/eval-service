# Benchmark and evidence interpretation

The private UI is an evidence viewer, not a leaderboard. Every number is
traceable to a run directory, a manifest, a trace, or the explicit
value `n/a`. This document defines the terms used by the UI and the
comparability rules.

For the storage contract see [`run-metrics.md`](run-metrics.md); for the
access path see [`private-access.md`](private-access.md); for the RAG
walkthrough see [`rag-verification.md`](rag-verification.md).

## Current vs legacy runs

A run is **current** when `manifest.json` exists and carries a model
(manifest model or a model recorded in the trial output). A run without
a manifest/model is **legacy**.

| Property | Current | Legacy |
|---|---|---|
| Manifest | present, validated | absent or incomplete |
| Model / lane / free-only | known and displayed | `n/a` |
| Default charts and benchmark matrix | included | excluded |
| Runs table | included, with a `legacy` badge | included |
| Benchmark rows | included only with “Include legacy runs” | opt-in |
| Comparable to current | yes, if lane/model/version match | **no** |
| Useful for | current evidence, trends, comparisons | historical inspection only |

Legacy runs are never backfilled by guessing. `eval manifest-backfill`
can add a manifest only from data the store already has; it does not
invent lane, model, or provenance. See [`legacy-runs.md`](legacy-runs.md).

## Manifest fields that matter

| Field | Meaning | Operator check |
|---|---|---|
| `lane` | `batch`, `local-service`, `remote-service`, `replay` | compare only same lane |
| `model.provider`, `model.model` | model under test | remote research is `target-service:remote` |
| `free_only` | only free models were eligible | must be `true` for production live |
| `target.version` | target project/version | same target for like-for-like |
| `eval_version` | platform commit/version | changes can change grading |
| `ref_name` | `live`, `research-service-remote`, ... | not a quality label by itself |
| `evidence.trace_id` | trace correlation | join to Phoenix |
| `evidence.s3` | mirror pointer | canonical copy is still the store |
| `provenance.origin` | `live` / `replay` / `synthetic` | replay never counts as live |

Unknown fields are displayed as `n/a`, never zero-filled.

## Model disaggregation

The benchmark table groups by target and model. Read it literally:

- A local research run has a real free model, e.g.
  `openrouter:nvidia/nemotron-3-ultra-550b-a55b:free`.
- A **remote** research run is `target-service:remote`. The deployed
  target does not expose its upstream LLM, so the UI records the service
  boundary, not a model name. Never claim a model comparison for
  `target-service:remote`.
- Digest runs show the target's selected free provider/model. The judge
  model is separate and is recorded in `judge.json`, never folded into
  the candidate model.

The model filter uses the displayed string. A filter for a specific
model excludes `target-service:remote` unless selected explicitly.

## Query filters

Research runs carry a `query`/`topic`; digest runs use the GitHub digest
label when no explicit query is stored. The Benchmark view filters by:

- repo group (`digest_agent` / `research_agents`),
- suite,
- query,
- model / target model,
- include-legacy toggle.

The query filter is exact-string. If a run has no query, it is grouped
as `no query` and will not match a specific query filter. Compare only
runs with the same query.

## Sources and citations

For research, `nsources` is the number of returned sources and
`citations` is the number of citation markers in the rendered answer.
`artifact_chars` is the answer size. The UI also exposes the source list
with title, provider, year, DOI/URL, and abstract excerpt.

Rules:

- `0` is a real measured zero (for example, zero sources returned).
- `n/a` means the run did not expose the field; it is not zero.
- A source count without citations is not evidence of grounding; read
  the answer and the source text.
- The Sources vs citations chart is diagnostic. A point at `(0,0)` is a
  failed grounding run, not “no data”.

The known remote research artifact wrapper returned 0 sources and empty
citation markers in earlier evidence. The suite failed closed. Do not
present that run as a good RAG example.

## Judge scores

The content judge is a **free-LLM advisory** artifact (`judge.json`).
Dimensions are:

- `grounding`: claims supported by the supplied result/sources,
- `relevance`: answers the query,
- `completeness`: covers the important parts,
- `clarity`: readable structure,
- `no_padding`: no filler/hype,
- `overall`: summary score.

The judge is not a gate and has no human-labelled calibration set.
Missing judge data is `n/a`. The digest judge reads the generated PDF;
the research judge reads the captured artifact/markdown. Do not compare
judge scores across suites, models, or rubric versions as if they were
the same instrument. See
[`judge-calibration.md`](judge-calibration.md).

## What is comparable

A comparison is valid only when all known values match:

- suite id and version,
- lane (`local-service` vs `remote-service` are different),
- free-only policy,
- model / target model,
- eval version, when both manifests record it,
- query/topic for research.

The UI marks a legacy run; the CLI `eval compare` refuses unlike-for-like
runs with exit `2` and writes no decision. “Not compared” is not a pass.

Not comparable examples:

- digest vs research,
- local research vs remote research,
- `target-service:remote` vs a named free model,
- current vs legacy,
- different RAG query,
- replay vs live.

## Reading a run

1. Open **Runs** and find the suite/ref, or use the latest card.
2. Check the header: lane, model, free-only, target version, trace.
3. Open **Cases** and read every failed check’s evidence.
4. Open **Query / Sources / Answer** for research; inspect the source
   cards and the answer body.
5. Open **Judge** and treat it as advisory.
6. Open **Files** for `manifest.json`, `metadata.json`, `grades.json`,
   `trace.json`, `judge.json`, and artifacts.
7. Cross-check S3 only after the local store; S3 is an append-only
   mirror.

API equivalents (session cookie plus Caddy Basic Auth; export
`BASIC_USER`/`BASIC_PASS` or use the browser):

```sh
curl -sS -u "$BASIC_USER:$BASIC_PASS" -b /tmp/eval-cookie \
  http://127.0.0.1:19090/api/runs | jq '.runs[0]'
curl -sS -u "$BASIC_USER:$BASIC_PASS" -b /tmp/eval-cookie \
  'http://127.0.0.1:19090/api/run?suite=research-agents-service&ref=research-service-remote&run_id=<id>' \
  | jq '{run: .run, query: .query}'
```

## Honest gaps

- Remote research model is opaque (`target-service:remote`); internal
  tokens/cost/spans are `n/a`.
- Research retrieval is external and can be flaky; a failure is
  evidence, not a reason to hide the run.
- The production research suite still hardcodes `crispr-offtarget`; the
  RAG topic is in the dataset and adapter default, but the suite must be
  updated before calling remote RAG production evidence.
- The judge is advisory and uncalibrated.
- Public benchmark page and private UI have different scopes: the public
  page selects the latest passing run per ref; the private UI shows
  current and legacy runs and is the authoritative evidence view.
