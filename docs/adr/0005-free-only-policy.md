# ADR 0005: Free-only means no paid model fallback

## Status

Accepted.

## Context

The project is a low-cost personal platform. Free-tier model availability is
not guaranteed, and a silent paid fallback would violate the operating budget.

## Decision

Production runs explicitly record free-only eligibility. The runner enforces a
free-model allowlist where applicable. The judge fails closed on a non-free
model or non-zero cost. If a free quota is exhausted, the run fails; it does not
fall back to a paid model.

## Consequences

- Costs stay bounded by infrastructure, not model spend.
- Runs can fail because of free-tier rate limits.
- The operator must distinguish infrastructure cost from model cost when
  reporting.
- Benchmarks must show the model/policy label, including `n/a` when the target
  does not expose it.
