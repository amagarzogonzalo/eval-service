# Observability stack

This stack is intentionally small enough for the 2 GB runner:

- Arize Phoenix: trace/span UI, backed by a Docker volume.
- OpenTelemetry Collector: OTLP HTTP/gRPC receiver and Phoenix exporter.
- Caddy: static benchmark site on loopback.

All ports bind to `127.0.0.1`. Access is through SSH local forwarding; the
security group does not expose a public unauthenticated UI.

## Start

```bash
sudo ROOT=/srv/eval-service /srv/eval-service/infra/observability/start.sh
```

## View

```bash
ssh -i ~/.ssh/eval-runner-aws \
  -L 6006:127.0.0.1:6006 \
  -L 8080:127.0.0.1:8080 \
  ubuntu@<runner-ip>
```

- Phoenix traces: <http://127.0.0.1:6006>
- Benchmark website: <http://127.0.0.1:8080>

## Export traces

Set on eval-service and the proxy sidecar:

```bash
EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328
```

The CLI writes `trace.json` beside each run and exports OTLP/HTTP JSON. The
proxy creates `gen_ai.chat` spans and forwards W3C `traceparent` context when
the target sends it.

## Retention and backups

- Phoenix data: Docker volume `eval-phoenix-data`.
- Back up with `docker run --rm -v eval-phoenix-data:/data -v "$PWD":/backup alpine tar czf /backup/phoenix.tgz -C /data .`.
- The canonical run evidence remains in `/var/lib/eval-runs` and S3.

## Capacity

Phoenix is the default because Langfuse's full stack is too heavy for this
node. If the runner is upgraded, Langfuse can replace Phoenix behind the same
OTLP collector boundary without changing eval-service.
