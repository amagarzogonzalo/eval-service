# OpenTelemetry GenAI mapping (pinned)

The canonical event contract is `llm/v1` (`crates/eval-usage`), versioned
independently. This document pins the mapping between that contract and
the OpenTelemetry GenAI semantic conventions **as observed in this
workspace** (`research-orchestration-agents/.../telemetry.py`,
`infrastructure/providers.py`).

GenAI conventions are developmental and their field names have changed
in the past. This mapping is versioned separately as `otel-mapping: 1`
below. If OTel renames attributes, this file gets a new version — the
`llm/v1` contract does not move.

## Mapping (otel-mapping: 1)

| `llm/v1` field | OTel GenAI equivalent (as observed) |
|---|---|
| `provider` | `gen_ai.system` |
| `model` | `gen_ai.request.model` |
| `input_tokens` | `gen_ai.usage.input_tokens` |
| `output_tokens` | `gen_ai.usage.output_tokens` |
| `duration_ms` | `gen_ai.client.operation.duration` (ms) |
| `outcome: ok` | span status `Ok` |
| `outcome: error/timeout` + `error_code` | span status `Error` + recorded exception |

## Deliberately not mapped

- `event_id`, `request_id`, `attempt`, `task_id`, `trace_id`: identity
  and association live in `llm/v1` only. OTel trace/span IDs may be
  copied into `trace_id` by an importer, but they are never required.
- `cost_usd`, `pricing_version`: costs are computed from the versioned
  pricing table at aggregation time, never trusted from spans
  (same rule as the sibling repo's ADR 0006).
- `ttft_ms`, `cached_tokens`, `reasoning_tokens`, `prompt_id`,
  `origin`, `context`, `tag`: no stable OTel equivalent pinned here;
  they travel in `extensions` if an importer has them under other names.
- `streaming`: inferred from the presence of `ttft_ms`, not mapped.

## Importing OTel data

An importer MAY translate OTel spans into `llm/v1` events using this
table, but MUST fill `event_id`, `project`, `request_id`, `provider`,
`model`, `started_at_ms`, `outcome`, and `context` itself — spans do
not reliably carry all of them. Anything unmappable stays absent
(never zero-filled, never guessed).
