# ADR 0004: Separate current and legacy evidence

## Status

Accepted.

## Context

Older runs predate model/provenance fields. Combining them with current runs in
benchmark charts silently mixes evidence with different guarantees.

## Decision

A run with a manifest and recorded model is current. A run without that data is
legacy. Legacy runs remain inspectable in the run explorer but are excluded
from default benchmark charts and model aggregates; an explicit toggle includes
them.

## Consequences

- Default charts describe comparable current evidence.
- Historical data is not destroyed or misrepresented.
- The UI must clearly badge legacy runs.
- The definition is implemented in the API and covered by tests.
