#!/usr/bin/env bash
# Reproducible black-box research service E2E.
#
#   run_live_service.sh local    start target uvicorn + free-model proxy
#   run_live_service.sh remote   use the deployed Render service
#
# Tokens are read from the environment or an env file and are never printed.
# Local model calls are pinned to a free OpenRouter model through eval-proxy;
# proxy usage is recorded and checked against a free-model allowlist.
# Remote needs EVAL_RESEARCH_SERVICE_TOKEN = the app API_TOKEN, not a Render
# rnd_ management key. Health/readiness can run without a token.
set -euo pipefail

MODE="${1:-remote}"
if [[ "$MODE" != "local" && "$MODE" != "remote" ]]; then
  echo "usage: $0 local|remote" >&2
  exit 2
fi
if [[ -n "${CI:-}" ]]; then
  echo "refusing to run live research service E2E in CI" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EVAL_BIN="${EVAL_BIN:-/srv/eval-service/target/release/eval}"
STORE="${EVAL_STORE:-/var/lib/eval-runs}"
S3_URI="${EVAL_S3_URI:-}"
SUITE="${EVAL_SERVICE_SUITE:-$ROOT/integrations/research_agents/live_service.yaml}"
REF="${EVAL_REF:-research-service-$MODE}"
LOCK="${EVAL_SERVICE_LOCK:-/tmp/eval-research-service.lock}"
ENV_FILE="${EVAL_ENV_FILE:-$ROOT/.env}"
ARTIFACT_OUT="${EVAL_RESEARCH_ARTIFACT_OUT:-$STORE/research-service-artifacts}"
OTEL_ENDPOINT="${EVAL_OTEL_ENDPOINT:-http://127.0.0.1:4328}"
FREE_MODEL="${EVAL_FREE_MODEL:-minimax/minimax-m3.1-flash:free}"
FREE_ALLOWLIST="${EVAL_FREE_MODEL_ALLOWLIST:-$FREE_MODEL}"
PROXY_PORT="${EVAL_PROXY_PORT:-8797}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
TAG="${EVAL_PROXY_TAG:-research-service-local-$STAMP}"
RESOLVER="$ROOT/integrations/research_agents/key_resolver.py"

log() { printf 'research-service: %s\n' "$*" >&2; }
die() { log "$*"; exit 3; }

resolve_key() {
  local name="$1"
  shift
  local python_bin="${PYTHON:-python3}"
  "$python_bin" "$RESOLVER" --name "$name" "$@"
}

[[ -x "$EVAL_BIN" ]] || die "eval binary not executable: $EVAL_BIN"
[[ -w "$STORE" ]] || die "store not writable by $(id -un): $STORE"
mkdir -p "$ARTIFACT_OUT"

exec 9>"$LOCK"
if ! flock -n 9; then
  die "another research service run holds $LOCK"
fi

PROXY_PID=""
SERVICE_PID=""
SERVICE_LOG="$STORE/research-service-local.log"
cleanup() {
  [[ -n "$SERVICE_PID" ]] && kill "$SERVICE_PID" 2>/dev/null || true
  [[ -n "$PROXY_PID" ]] && kill "$PROXY_PID" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT

if [[ "$MODE" == "local" ]]; then
  : "${EVAL_RESEARCH_PYTHON:?set EVAL_RESEARCH_PYTHON}"
  : "${EVAL_RESEARCH_REPO:?set EVAL_RESEARCH_REPO}"
  OPENROUTER_FILES=(--file "$ENV_FILE")
  if [[ -n "${EVAL_OPENROUTER_ENV_FILE:-}" ]]; then
    OPENROUTER_FILES=(--file "$EVAL_OPENROUTER_ENV_FILE" "${OPENROUTER_FILES[@]}")
  fi
  OPENROUTER_API_KEY="$(resolve_key OPENROUTER_API_KEY "${OPENROUTER_FILES[@]}" \
    --opencode-provider openrouter)" || die "OpenRouter key not found"
  export OPENROUTER_API_KEY
  SERVICE_TOKEN="$(openssl rand -hex 24)"
  SERVICE_PORT="$("$EVAL_RESEARCH_PYTHON" - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

  log "starting free-model proxy on 127.0.0.1:$PROXY_PORT ($FREE_MODEL)"
  "$EVAL_BIN" proxy \
    --port "$PROXY_PORT" \
    --key-env OPENROUTER_API_KEY \
    --project research-agents \
    --tag "$TAG" \
    --store "$STORE" \
    --provider openrouter \
    --max-requests "${EVAL_MAX_REQUESTS:-80}" \
    --upstream-timeout-s 180 \
    --override "openai/gpt-4o-mini=$FREE_MODEL" \
    --override "openrouter/openai/gpt-4o-mini=$FREE_MODEL" \
    --override "gpt-4o-mini=$FREE_MODEL" \
    https://openrouter.ai/api/v1 \
    >"$STORE/research-service-proxy.log" 2>&1 &
  PROXY_PID=$!
  for _ in $(seq 1 40); do
    if curl -s -o /dev/null "http://127.0.0.1:$PROXY_PORT/"; then
      break
    fi
    sleep 0.5
  done
  if ! curl -s -o /dev/null "http://127.0.0.1:$PROXY_PORT/"; then
    die "free-model proxy failed; see $STORE/research-service-proxy.log"
  fi

  log "starting target uvicorn on 127.0.0.1:$SERVICE_PORT"
  (
    export PYTHONPATH="$EVAL_RESEARCH_REPO/src:$EVAL_RESEARCH_REPO"
    export PYTHONDONTWRITEBYTECODE=1
    export JOBS_KIND=memory
    export API_TOKEN="$SERVICE_TOKEN"
    export ALLOW_ANONYMOUS=0
    export MAX_CONCURRENT_JOBS=1
    export MAX_QUEUED_JOBS=1
    export MAX_ACTIVE_JOBS_PER_CALLER=1
    export RATE_LIMIT_PER_MINUTE=0
    export JOB_TIMEOUT_SECONDS=1200
    export CLEANUP_AFTER_RUN=false
    export ARTIFACT_STORE_KIND=local
    export CHECKPOINTER_KIND=file
    export STORE_KIND=memory
    export WIKI_STORE_KIND=local
    export OPENROUTER_API_KEY=dummy-eval-proxy
    export OPENROUTER_API_BASE="http://127.0.0.1:$PROXY_PORT/v1"
    export GROQ_API_KEY=
    export GEMINI_API_KEY=
    export MISTRAL_API_KEY=
    export OTEL_TRACES_EXPORTER=otlp
    export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_ENDPOINT%/}/v1/traces"
    export OTEL_SERVICE_NAME=research-orchestration-agents-local
    export OTEL_RESOURCE_ATTRIBUTES="eval.lane=service_local,service.run_tag=$TAG"
    exec "$EVAL_RESEARCH_PYTHON" -m uvicorn knowledge_engine.service.api:app \
      --host 127.0.0.1 --port "$SERVICE_PORT"
  ) >"$SERVICE_LOG" 2>&1 &
  SERVICE_PID=$!
  for _ in $(seq 1 120); do
    if curl -s -o /dev/null "http://127.0.0.1:$SERVICE_PORT/health"; then
      break
    fi
    if ! kill -0 "$SERVICE_PID" 2>/dev/null; then
      tail -40 "$SERVICE_LOG" >&2 || true
      die "target uvicorn exited"
    fi
    sleep 0.5
  done
  if ! curl -s -o /dev/null "http://127.0.0.1:$SERVICE_PORT/health"; then
    tail -40 "$SERVICE_LOG" >&2 || true
    die "target uvicorn did not become healthy"
  fi

  export EVAL_RESEARCH_SERVICE_URL="http://127.0.0.1:$SERVICE_PORT"
  export EVAL_RESEARCH_SERVICE_LANE=local
  export EVAL_RESEARCH_SERVICE_TOKEN="$SERVICE_TOKEN"
  export EVAL_RESEARCH_USAGE_STORE="$STORE"
  export EVAL_RESEARCH_USAGE_PROJECT=research-agents
  export EVAL_RESEARCH_USAGE_TAG="$TAG"
  export EVAL_RESEARCH_REQUIRE_FREE_MODELS="${EVAL_RESEARCH_REQUIRE_FREE_MODELS:-1}"
  export EVAL_FREE_MODEL="$FREE_MODEL"
  export EVAL_FREE_MODEL_ALLOWLIST="$FREE_ALLOWLIST"
  export EVAL_RESEARCH_ARTIFACT_OUT="$ARTIFACT_OUT"
  export EVAL_RESEARCH_TRACE_ENDPOINT="${OTEL_ENDPOINT%/}/v1/traces"
  export EVAL_OTEL_ENDPOINT="$OTEL_ENDPOINT"
else
  SERVICE_URL="${EVAL_RESEARCH_SERVICE_URL:-https://research-orchestration-agents.onrender.com}"
  if [[ -z "${EVAL_RESEARCH_SERVICE_TOKEN:-}" ]]; then
    EVAL_RESEARCH_SERVICE_TOKEN="$(resolve_key EVAL_RESEARCH_SERVICE_TOKEN \
      --file "$ENV_FILE" 2>/dev/null || true)"
    export EVAL_RESEARCH_SERVICE_TOKEN
  fi
  if [[ "${EVAL_RESEARCH_SERVICE_TOKEN:-}" == rnd_* ]]; then
    die "EVAL_RESEARCH_SERVICE_TOKEN looks like a Render API key; use the app API_TOKEN"
  fi
  export EVAL_RESEARCH_SERVICE_URL="$SERVICE_URL"
  export EVAL_RESEARCH_SERVICE_LANE=remote
  export EVAL_RESEARCH_REQUIRE_FREE_MODELS=0
  export EVAL_RESEARCH_ARTIFACT_OUT="$ARTIFACT_OUT"
  export EVAL_OTEL_ENDPOINT="$OTEL_ENDPOINT"
  log "health/readiness probe: $SERVICE_URL"
  "$EVAL_BIN" check "$SUITE" --probe || die "service health/readiness probe failed"
  if [[ -z "${EVAL_RESEARCH_SERVICE_TOKEN:-}" ]]; then
    die "full service run blocked: set EVAL_RESEARCH_SERVICE_TOKEN (the app API_TOKEN)"
  fi
fi

log "running $SUITE (ref $REF, lane $MODE)"
"$EVAL_BIN" run "$SUITE" \
  --ref "$REF" \
  --origin live \
  --workers 1 \
  --store "$STORE" \
  --json | tee "$STORE/research-service-last.json"

RUN_ID="$(sed -n 's/.*"run_id": "\([^"]*\)".*/\1/p' "$STORE/research-service-last.json" | head -n1)"
TRACE_FILE="$STORE/runs/research-agents-service/$REF/run-$RUN_ID/trace.json"
if [[ -f "$TRACE_FILE" ]]; then
  TRACE_ID="$(sed -n 's/.*"trace_id": "\([^"]*\)".*/\1/p' "$TRACE_FILE" | head -n1)"
  log "run=$RUN_ID trace=$TRACE_ID file=$TRACE_FILE"
fi

if [[ -n "$S3_URI" ]]; then
  aws s3 sync "$STORE" "$S3_URI" --only-show-errors
fi
