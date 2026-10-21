# Benchmark method

This page defines what the private benchmark page and judge mean. It is a
reporting contract, not a claim that every target is statistically calibrated.

## Run identity

A run is identified by `suite + ref + run_id`. The canonical evidence is the
run directory under `/var/lib/eval-runs`. S3 is a durable mirror, not a
separate source of truth.

A current run has a valid `manifest.json` with a recorded model. Runs without
model/provenance data are **legacy**: they remain visible in the run explorer
but are excluded from default benchmark charts and model aggregates.

## Metrics

| Metric | Definition |
|---|---|
| Pass rate | Passed cases / total cases in the run |
| p50 | Median observed trial wall time |
| p95 | 95th-percentile observed trial wall time |
| Trials/min | Completed trials / summed trial wall time |
| Sources | Source records persisted by the target |
| Citations | Unique citation markers in the artifact |
| Judge | Advisory rubric score, shown with its dimensions and issues |

Unavailable measurements are `n/a`, never zero. A zero is only valid when the
measurement was genuinely observed as zero.

## Disaggregation

The benchmark can be filtered by repository, suite, query, model/target, and
current-vs-legacy. Model cards and the disaggregated table show the latest
judge score, pass rate, p50, and source count per target/model group.

For the remote research target, `target-service:remote` is an honest boundary
label: the target service does not expose its upstream LLM. For digest runs,
the model is recorded from the run manifest (for example `gemini:free`).

## Judge

The judge is advisory. It receives the query and the captured result, then
returns five 0–10 dimensions plus an overall score, rationale, and issues. It
fails closed on non-free models, malformed JSON, or non-zero cost.

Do not present judge scores as ground truth. Use them alongside deterministic
checks and human inspection. A human calibration set is still pending.

## Comparability rules

- Compare like-for-like suites, queries, lanes, and model policies.
- Do not compare legacy runs to manifested runs without opening the run.
- A run with one or two trials is evidence, not a statistically strong sample.
- A failed target run is a real result; do not filter it out of pass-rate
  calculations.
- Re-running a query can produce a different answer. Record the run ID.
- Remote and local lanes are not interchangeable.

## RAG inspection

For the RAG benchmark, open the research run and verify:

1. The query matches the intended RAG question.
2. The answer is substantive and addresses retrieval, grounding, and
   hallucination reduction.
3. Citations point to the claims they support.
4. The source list is relevant, not merely numerous.
5. Deterministic checks passed.
6. The judge's issues are consistent with what you read.

The current production query is:

> How retrieval-augmented generation reduces hallucinations in large language
> models
