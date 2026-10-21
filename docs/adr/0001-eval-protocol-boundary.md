# ADR 0001: Evaluate through a versioned protocol boundary

## Status

Accepted.

## Context

Targets use different languages and frameworks. Importing target code into the
engine would couple evaluation to implementation details and make each new
project a platform change.

## Decision

The engine launches each target adapter as a subprocess. Communication uses the
versioned `eval/v1` JSONL protocol. Adapters live under `integrations/`; target
repositories are never modified.

## Consequences

- Adding a project is an adapter + suite change, not an engine change.
- The engine can test local, remote, and replayed targets with one contract.
- Protocol changes require versioning and conformance tests.
- The subprocess boundary is not an OS sandbox; untrusted targets need the
  Docker sandbox.
