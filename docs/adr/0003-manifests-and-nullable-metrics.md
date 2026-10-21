# ADR 0003: Manifests and nullable metrics are part of the contract

## Status

Accepted.

## Context

A score without provenance cannot be compared or audited. Absent measurements
are often silently coerced to zero, which makes missing data look like a real
result.

## Decision

Every current run writes a `run-manifest/v1` with target, lane, model, budgets,
metrics, evidence pointers, and provenance. Metrics use nullable values
throughout. Reports render `n/a` for unobserved data; zero is reserved for a
genuine observation.

## Consequences

- Run comparisons are auditable.
- Legacy runs without manifests are visibly separated.
- Consumers must handle null instead of assuming defaults.
- The generated manifest schema is checked in and tested.
