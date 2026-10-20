# eval-proxy: observing real LLM calls without touching target repos

`eval proxy` serves a **loopback reverse proxy** that speaks the
OpenAI-compatible `chat/completions` endpoint. A target application
points its provider base URL at the proxy; the proxy injects the real
provider key, optionally swaps models, relays responses (non-streaming
and SSE), enforces a budget, and records one `llm/v1` event per
physical attempt in the usage store.

This is the supported interception seam for projects whose own provider
adapters do not expose a base-URL override:

- The application sees `OPENAI_BASE_URL=<proxy>` and at most a **dummy
  key** — real credentials exist only in the proxy's process.
- The proxy is the one place that can see model, request shape, tokens
  (provider-reported only), latency, and errors per call.
- TLS interception and runtime monkey-patching are deliberately
  **prohibited**; if a target has no base-URL seam at all, the per-
  project adapter injects a provider port instead (see
  `integrations/digest_agent/live_provider.py` for the pattern).

## Usage

```sh
eval proxy \
    --key-env GROQ_API_KEY \
    --project digest-agent \
    --tag digest-live-proxy \
    --max-requests 30 \
    --max-cost-usd 0.50 \
    --price-in-per-1m 0.075 --price-out-per-1m 0.30 \
    -- https://api.groq.com/openai/v1
```

Any OpenAI-compatible upstream works; for example OpenRouter uses
`--provider openrouter -- https://openrouter.ai/api/v1` and its own
rate card (add matching rates to `eval-runs/pricing.toml`).

Then run the target with `EVAL_*_ENDPOINT=http://127.0.0.1:8787/v1`
and a dummy key. Digest agent worked example (adapter already wired):

```sh
EVAL_DIGEST_LIVE=1 \
EVAL_DIGEST_API_KEY=dummy-eval-proxy-observes \
EVAL_DIGEST_ENDPOINT=http://127.0.0.1:8787/v1 \
EVAL_DIGEST_MODEL=openai/gpt-oss-20b \
eval run integrations/digest_agent/live_bench.yaml --ref live-oss20b --workers 1
```

Use `--workers 1` for live suites: independent cases still burst the
provider's rate-limit window when run concurrently.

Every event lands directly in the store; inspect with
`eval usage-report --project digest-agent --tag digest-live-proxy`.

## Cassettes: record once, replay offline

`eval proxy ... --cassette-dir DIR` records every successful (HTTP 200)
exchange as a JSON cassette keyed by a blake3 fingerprint of the
application's exact request body. Authorization headers and key
material are never written. Replay serves those responses offline:

```sh
eval cassette DIR --project digest-agent --port 8788
```

Point the application at `http://127.0.0.1:8788/v1` with a dummy key.
Replay is honest by construction: an unmatched request is a 404
`cassette_miss` (never a plausible fallback), the requested streaming
mode must match the recording, and every hit stamps `origin: replay`
with the recorded call's own usage block — so replay events stay
labeled as replay in `eval usage-report`. Replay output must never be
promoted as live evidence: a live gate requires a fresh `eval proxy`
run (`origin: live`).

## Budget semantics (honest)

- `--max-requests N`: at most N forwarded attempts; request N+1 gets
  HTTP 429 `request_budget_exhausted` and is never forwarded.
- `--max-cost-usd X`: requires **both** per-1M prices; the proxy
  refuses to start otherwise (fail closed — an unpriced cost budget
  cannot bound real spend).
- Before forwarding, each request reserves a conservative cost band:
  input tokens are estimated **high** from wire size (`bytes/3 + 32`)
  and output from the request's `max_tokens` (or the configured
  reserve). Reservations are atomic — concurrent requests cannot
  jointly exceed the cap.
- After the attempt, the reservation is reconciled against
  provider-reported usage. When usage is unobservable (e.g. an
  interrupted stream), the reservation itself is spent on the ledger —
  unknown use is never accounted as free.
- Rationale: usage arrives only with the final response chunk, so
  reservation-before-request is the only honest way to bound spend
  mid-flight. The bound has documented slack; it is an
  application-side control, **not a billing guarantee**. Reservation
  state is internal — it never appears as a metric or in reports.

## Event semantics

- One `llm/v1` event per physical attempt: `outcome` from the HTTP
  status (`ok` / `error` / `upstream_timeout`), real latency, and
  tokens **only** when the provider's `usage` block reports them —
  absent stays absent, never zero. Non-200 upstream responses pass
  through unchanged to the caller.
- Streaming requests get `stream_options.include_usage` added when
  the caller did not opt in, and `ttft_ms` is observed from the first
  upstream chunk. The response bytes are relayed verbatim (chunked).
- Model swaps stamp the served model on the event and record the
  requested one under `extensions.requested_model`.
- `origin` is always `live` from this path; cassette replay stamps
  `replay` and must never satisfy a live gate.
- Beside the digest flow, do **not** also set `EVAL_USAGE_OUT` for the
  same run: that would double-record (app-side decorator + proxy).
  Pick one observer per run; the proxy is the preferred one for its
  provider-agnostic view.

## Limits

- Binds loopback by default. `--bind 0.0.0.0` exists only for the
  Docker **sidecar** network in `docs/sandbox.md`; never expose it on a
  public interface.
- `cost_usd` on events stays `n/a`; cost accounting is estimated at
  aggregation time from a versioned pricing table (provenance-visible
  in `eval usage-report`). Provider-reported per-call billing is
  honored when a provider returns it.
- No retries are issued by the proxy itself: one request → one
  forwarded attempt → one event.
