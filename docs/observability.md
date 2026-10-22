# Observability

eval-service keeps its canonical run store in `eval-runs/` and S3. Traces
are an observability layer: they must never decide a gate and must not be
required for a run to succeed.

## Stack

- **Arize Phoenix** — trace/span UI and trace API.
- **OpenTelemetry Collector** — receives OTLP HTTP/gRPC and exports to
  Phoenix.
- **Caddy** — private evidence UI on loopback (`8080` -> `8090`).
- **SSH forwarding** — access; no unauthenticated public UI.

Phoenix is the default because the runner is a 2 GB `t4g.small`.
Langfuse's full self-hosted stack is too large for this node. The OTLP
boundary means Langfuse can be added later without changing eval-service.

## Trace model

One evaluation run is one trace:

```
eval.run
├── eval.case
│   └── eval.trial
├── gen_ai.chat        (proxy, one per upstream attempt)
└── eval.grader / eval.gate
```

Required attributes:

- `eval.run_id`, `eval.suite`, `eval.target`
- `eval.origin` (`live` / `replay`), `eval.sandbox`
- `eval.case_id`, `eval.trial`
- `gen_ai.request.model`, `gen_ai.response.model`
- `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`
- `eval.wall_ms`, `eval.status`, `eval.pass`, `eval.retry`

The CLI writes `trace.json` beside `metadata.json`; it contains the W3C
trace ID and `traceparent`. The runner injects `TRACEPARENT` and
`EVAL_TRACE_ID` into target processes. The proxy accepts `traceparent`
from the target and creates a child `gen_ai.chat` span.

## Export

On the VM:

```bash
EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328
```

Inside the proxy sidecar, use the host gateway:

```bash
EVAL_OTEL_ENDPOINT=http://host.docker.internal:4328
```

Export is best-effort: a collector outage logs a warning and never
changes a run result. Traces contain no provider keys, prompts or
responses by default.

## Ports

| Service | Host binding | Purpose |
|---|---|---|
| Phoenix UI | `127.0.0.1:6006` | traces |
| OTel HTTP | `127.0.0.1:4328` | OTLP/HTTP JSON |
| OTel gRPC | `127.0.0.1:4327` | OTLP/gRPC |
| Private UI | `127.0.0.1:8090` | read-only evidence API/UI |
| Private Caddy | `127.0.0.1:8080` | Basic Auth in front of `8090` |

Ports 4317/4318 are intentionally avoided because Agent C's Jaeger
container owns them during the research workstream.

## Access

```bash
ssh -i ~/.ssh/eval-runner-aws \
  -L 6006:127.0.0.1:6006 \
  -L 19090:127.0.0.1:8080 \
  ubuntu@<runner-ip>
```

Then open `http://127.0.0.1:6006` for Phoenix and
`http://127.0.0.1:19090` for the private UI. Authentication is covered
in [`private-access.md`](private-access.md); public/static benchmark
hosting must not reuse the private listener (see [`web.md`](web.md)).

## Retention

- Phoenix data is in Docker volume `eval-phoenix-data`.
- Canonical artifacts remain in `/var/lib/eval-runs` and S3.
- Back up the volume before rebuilding the stack.
- Store retention policy applies to traces and the private evidence view.

## Not a gate

The PR gate is replay-only and has no collector dependency. A failed
collector or Phoenix container must leave the evaluation run, S3 sync and
watcher healthy.
