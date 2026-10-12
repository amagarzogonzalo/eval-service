# LLM usage event contract (`llm/v1`)

One record per physical model-call attempt, reported by instrumented
projects through a common interface. Language-independent: any project
that can write JSONL (or POST JSON) can report usage without adopting
the platform's Rust runtime or any provider SDK.

## Event shape

Required: `event` (`"llm.call"`), `event_version` (`1`), `event_id`
(idempotency key), `project`, `request_id`, `provider`, `model`,
`started_at_ms` (nonzero), `outcome` (`ok`/`error`/`timeout`),
`context` (`production`/`benchmark`/`dev`).

Optional (absent = unknown, never zero): `trace_id`, `task_id`,
`attempt` (default 0), `duration_ms`, `streaming`, `ttft_ms`,
`input_tokens`, `output_tokens`, `cached_tokens`, `reasoning_tokens`,
`error_code`, `retriable`, `cost_usd` + `cost_provenance` (`reported`
only — local estimates are computed at aggregation, never stored on
events), `prompt_id`, `prompt_hash`, `run_id`, `case_id`, `tag`,
`extensions` (namespaced provider extras, never credentials).

`origin` (`live`/`replay`/`synthetic`, default `live`) labels fixture
data end to end. Unknown fields are tolerated on read; unknown event
versions and empty ids are rejected with reasons.

## Identity and retry semantics

- `request_id`: stable logical request. Retries share it.
- `attempt`: zero-based physical attempt within the request.
- `event_id`: unique per attempt. Re-ingesting an id is a no-op
  duplicate — every physical attempt is represented at most once,
  while attempts stay associated to their request (`request_id`) and
  task (`task_id`).
- Aggregation counts logical requests once for success rates
  (latest attempt decides) and every attempt for consumption
  (providers bill retries).

## Contexts never mix silently

`production`, `benchmark`, and `dev` traffic aggregate separately
(`eval usage-report --context …`). The default view sums all
contexts and labels itself as such.

## Cost provenance

- `reported`: provider-reported cost stored on the event.
- `estimated`: computed at aggregation from the versioned pricing
  table; the aggregate records the `pricing_version` used.
- Re-pricing the same events under another table version yields
  different estimates with identical raw events — history is never
  rewritten. Changing prices is a new table version, not an edit.

## Coexistence with legacy `AiUsage`

Result messages may still carry the old `AiUsage` block (model +
tokens + cost attached to a terminal result). Precedence rule: usage
views aggregate `llm/v1` events ONLY. Legacy blocks remain visible
inside run views (flagged `has_legacy_ai`) and are never added to
event totals — per-call measurements and aggregate result
annotations describe different things, and summing them would count
usage twice.

## What integrations must document

Per integration, state exactly how each field is obtained:

| Integration | Calls/latency/errors | Tokens | Method |
|---|---|---|---|
| `digest_agent` driver | Measured around the port | Unavailable (port returns `str`; provider usage discarded upstream) | Decorator, no project changes |
| `llm_calls` live | Measured | Parsed from provider `usage` blocks | Extended live path |
| Fixture benchmarks | Measured (local) | `len // 4` heuristic over real fixture text, labeled synthetic | Documented demo only |

Do not infer or fabricate tokens. If the integration cannot observe
them, the fields stay absent.

## OTel mapping

Pinned separately in `docs/otel-mapping.md` (mapping version 1).
The canonical contract is versioned independently and never tracks
OTel renames.
