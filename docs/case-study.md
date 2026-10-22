# Case study: making agent behavior measurable

## Problem

An agent can produce an impressive answer and still be wrong, unreproducible,
too slow, or dependent on a provider that is not allowed. A demo alone does not
show whether a change improved anything.

`eval-service` turns an agent run into evidence that can be compared:

- what was asked;
- which target and model policy ran;
- what the target returned;
- which deterministic checks passed;
- how long it took and what it consumed;
- what sources it used;
- what an advisory judge thought, and why;
- whether the result is current or legacy evidence.

## Constraints

- Targets keep their own repositories and implementation.
- The engine must not import target code.
- Free-only means no paid LLM fallback, not “no infrastructure.”
- Missing measurements are `n/a`, never zero.
- The UI is private, single-user, and read-only.
- Production runs are scheduled weekly, not triggered by ad-hoc browser use.

## Design

The engine owns a versioned subprocess protocol (`eval/v1`). Each target has a
thin adapter. The engine records observations and a manifest, applies
deterministic graders, and optionally runs a free judge. The private UI reads
the resulting store and exposes query, sources, answer, checks, judge, traces,
and raw files.

The key design choice is separation:

- deterministic checks gate;
- the judge explains quality and is advisory;
- the run manifest records provenance;
- S3 mirrors the canonical local store;
- current runs are separated from legacy runs in benchmarks.

See [architecture](architecture.md) and [benchmark method](benchmark-method.md).

## Evidence

A recent RAG research run used the query:

> How retrieval-augmented generation reduces hallucinations in large language
> models

Observed result:

| Measurement | Value |
|---|---|
| Cases | 2/2 passed |
| Sources | 50 |
| Citations | 7 |
| p50 latency | 89,029 ms |
| Throughput | 0.87 trials/min |
| Advisory judge | 9/10 |
| Target model policy | `target-service:remote` (upstream LLM not exposed) |

The answer and every source are visible in the private UI. The judge card shows
grounding, relevance, completeness, clarity, and no-padding scores plus its
issues.

These are illustrative observations from a real run, not a claim of universal
model quality or statistical significance.

## What this demonstrates

- Platform engineering across Rust, Python, CI, storage, and a private UI.
- Evaluation design: deterministic gates, advisory judging, provenance, honest
  missing data, and current/legacy separation.
- Operational discipline: weekly self-hosted runs, S3 evidence, free-only
  policy, and explicit known limitations.
- Product judgment: the result is inspectable by a human, not just a score.

## Limitations

- Single-user access, not multi-tenant production.
- No calibrated human-label set yet.
- The remote research target does not expose its upstream LLM.
- Target runs can be flaky; failures are reported rather than hidden.
- The private UI is reached through a tunnel, not a public ingress.
- Infrastructure is not free; AWS credits currently cover it.

## Next steps

1. Human calibration set and judge agreement reporting.
2. True multi-model comparison when the target exposes model identity.
3. Restore drills and retention automation.
4. Public case-study page and a short demo video.
