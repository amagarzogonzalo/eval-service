# ADR 0002: Deterministic checks gate; the judge is advisory

## Status

Accepted.

## Context

LLM judges are useful for qualities that are hard to express deterministically,
but they are nondeterministic and can be wrong. Treating a judge score as a
release gate would make the system opaque and difficult to calibrate.

## Decision

Deterministic checks decide pass/fail. The free content judge runs separately,
records its dimensions, rationale, issues, and usage, and is displayed as
advisory. The judge fails closed on non-free models, malformed JSON, or
non-zero cost.

## Consequences

- Runs remain reproducible and explainable.
- Judge quality can improve without changing release semantics.
- A human calibration set is still required before making stronger claims.
- Benchmarks must show judge scores alongside the deterministic result.
