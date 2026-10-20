#!/usr/bin/env bash
set -euo pipefail

ROOT=${ROOT:-/srv/eval-service}
DATA=${DATA:-/srv/observability}
WEB=${WEB:-/srv/bench-web}

install -d -m 755 "$DATA" "$WEB"
install -m 644 "$ROOT/infra/observability/otel-collector.yaml" "$DATA/otel-collector.yaml"
install -m 644 "$ROOT/infra/observability/Caddyfile" "$DATA/Caddyfile"

docker network inspect eval-observability >/dev/null 2>&1 \
  || docker network create eval-observability >/dev/null
docker volume inspect eval-phoenix-data >/dev/null 2>&1 \
  || docker volume create eval-phoenix-data >/dev/null

docker rm -f eval-phoenix eval-otel-collector eval-bench-web >/dev/null 2>&1 || true

docker run -d --name eval-phoenix \
  --restart unless-stopped \
  --network eval-observability --ip 172.19.0.2 \
  -p 127.0.0.1:6006:6006 \
  -e PHOENIX_WORKING_DIR=/data \
  -v eval-phoenix-data:/data \
  --memory 700m --memory-swap 1g \
  arizephoenix/phoenix:latest

docker run -d --name eval-otel-collector \
  --restart unless-stopped \
  --network eval-observability \
  -p 127.0.0.1:4328:4318 \
  -p 127.0.0.1:4327:4317 \
  -v "$DATA/otel-collector.yaml:/etc/otelcol/config.yaml:ro" \
  --memory 192m --memory-swap 256m \
  otel/opentelemetry-collector-contrib:latest \
  --config=/etc/otelcol/config.yaml

docker run -d --name eval-bench-web \
  --restart unless-stopped \
  --network host \
  -v "$WEB:/srv/bench-web:ro" \
  -v "$DATA/Caddyfile:/etc/caddy/Caddyfile:ro" \
  --memory 96m --memory-swap 128m \
  caddy:2

echo "phoenix: http://127.0.0.1:6006"
echo "bench web: http://127.0.0.1:8080"
echo "otlp: http://127.0.0.1:4328/v1/traces"
