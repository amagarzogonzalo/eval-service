#!/usr/bin/env bash
# Reproducible real E2E run for research-orchestration-agents.
#
# - refuses CI
# - exclusive lock against overlapping runs
# - provider key from SSM or the environment; never printed
# - starts eval proxy with request/cost caps if none is listening
# - propagates a W3C traceparent and runs one worker
# - stores locally and syncs to S3
#
# Required:
#   EVAL_RESEARCH_PYTHON  target venv python
#   EVAL_RESEARCH_REPO    target checkout (never edited)
# Optional:
#   EVAL_BIN, EVAL_STORE, EVAL_S3_URI, EVAL_PROXY_PORT, EVAL_PROXY_TAG,
#   EVAL_RESEARCH_MODEL, EVAL_RESEARCH_TRACE_ENDPOINT,
#   EVAL_RESEARCH_ARTIFACT_OUT, EVAL_MAX_REQUESTS, EVAL_MAX_COST_USD,
#   EVAL_SSM_KEY_PARAM, EVAL_LOCK
set -euo pipefail

if [[ -n "${CI:-}" ]]; then
  echo "refusing to run live research E2E in CI" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EVAL_BIN="${EVAL_BIN:-/srv/eval-service/target/release/eval}"
STORE="${EVAL_STORE:-/var/lib/eval-runs}"
PROXY_PORT="${EVAL_PROXY_PORT:-8796}"
PROXY_TAG="${EVAL_PROXY_TAG:-research-e2e}"
MODEL="${EVAL_RESEARCH_MODEL:-openai/gpt-4o-mini}"
OTEL_ENDPOINT="${EVAL_OTEL_ENDPOINT:-http://127.0.0.1:4328}"
TRACE_ENDPOINT="${EVAL_RESEARCH_TRACE_ENDPOINT:-${OTEL_ENDPOINT%/}/v1/traces}"
ARTIFACT_OUT="${EVAL_RESEARCH_ARTIFACT_OUT:-$STORE/research-e2e-artifacts}"
REF="${EVAL_REF:-research-e2e-live}"
S3_URI="${EVAL_S3_URI:-}"
MAX_REQUESTS="${EVAL_MAX_REQUESTS:-80}"
MAX_COST="${EVAL_MAX_COST_USD:-0.25}"
KEY_PARAM="${EVAL_SSM_KEY_PARAM:-/eval/openrouter_api_key}"
LOCK="${EVAL_LOCK:-/tmp/eval-research-live.lock}"

: "${EVAL_RESEARCH_PYTHON:?set EVAL_RESEARCH_PYTHON to the target venv python}"
: "${EVAL_RESEARCH_REPO:?set EVAL_RESEARCH_REPO to the target checkout}"

if [[ ! -x "$EVAL_BIN" ]]; then
  echo "eval binary not executable: $EVAL_BIN" >&2
  exit 2
fi
if [[ ! -w "$STORE" ]]; then
  echo "store not writable by $(id -un): $STORE (run as eval)" >&2
  exit 2
fi
mkdir -p "$ARTIFACT_OUT"

exec 9>"$LOCK"
if ! flock -n 9; then
  echo "another research E2E run holds $LOCK" >&2
  exit 3
fi

proxy_pid=""
cleanup() {
  if [[ -n "$proxy_pid" ]]; then
    kill "$proxy_pid" 2>/dev/null || true
    wait "$proxy_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

if curl -s -o /dev/null "http://127.0.0.1:$PROXY_PORT/"; then
  if [[ "${EVAL_REUSE_PROXY:-1}" != "1" ]]; then
    echo "port $PROXY_PORT already has a listener; set EVAL_REUSE_PROXY=1 to reuse it" >&2
    exit 3
  fi
  echo "reusing proxy on 127.0.0.1:$PROXY_PORT (caps are the listener's)" >&2
else
  if [[ -z "${OPENROUTER_API_KEY:-}" ]]; then
    OPENROUTER_API_KEY="$(aws ssm get-parameter --name "$KEY_PARAM" \
      --with-decryption --query Parameter.Value --output text)"
    export OPENROUTER_API_KEY
  fi
  echo "starting eval proxy on 127.0.0.1:$PROXY_PORT (project research-agents)" >&2
  "$EVAL_BIN" proxy \
    --port "$PROXY_PORT" \
    --key-env OPENROUTER_API_KEY \
    --project research-agents \
    --tag "$PROXY_TAG" \
    --store "$STORE" \
    --provider openrouter \
    --max-requests "$MAX_REQUESTS" \
    --max-cost-usd "$MAX_COST" \
    --price-in-per-1m 0.15 \
    --price-out-per-1m 0.60 \
    --upstream-timeout-s 180 \
    https://openrouter.ai/api/v1 \
    >"$STORE/research-e2e-proxy.log" 2>&1 &
  proxy_pid=$!
  for _ in $(seq 1 30); do
    if curl -s -o /dev/null "http://127.0.0.1:$PROXY_PORT/"; then
      break
    fi
    sleep 0.5
  done
  if ! curl -s -o /dev/null "http://127.0.0.1:$PROXY_PORT/"; then
    echo "proxy failed to start; see $STORE/research-e2e-proxy.log" >&2
    exit 2
  fi
fi

export EVAL_RESEARCH_PYTHON
export EVAL_RESEARCH_REPO
export EVAL_RESEARCH_ENDPOINT="http://127.0.0.1:$PROXY_PORT/v1"
export EVAL_RESEARCH_MODEL="$MODEL"
export EVAL_RESEARCH_TRACE_ENDPOINT="$TRACE_ENDPOINT"
export EVAL_RESEARCH_ARTIFACT_OUT="$ARTIFACT_OUT"
export EVAL_OTEL_ENDPOINT="$OTEL_ENDPOINT"

echo "store=$STORE model=$MODEL max_requests=$MAX_REQUESTS max_cost_usd=$MAX_COST" >&2
echo "otel=$OTEL_ENDPOINT target_trace=$TRACE_ENDPOINT" >&2

"$EVAL_BIN" run "$ROOT/integrations/research_agents/live_e2e.yaml" \
  --ref "$REF" \
  --origin live \
  --workers 1 \
  --store "$STORE" \
  --json | tee "$STORE/research-e2e-last.json"

run_id="$(sed -n 's/.*"run_id": "\([^"]*\)".*/\1/p' "$STORE/research-e2e-last.json" | head -n1)"
trace_file="$STORE/runs/research-agents-e2e/$REF/run-$run_id/trace.json"
if [[ -f "$trace_file" ]]; then
  trace_id="$(sed -n 's/.*"trace_id": "\([^"]*\)".*/\1/p' "$trace_file" | head -n1)"
  echo "trace=$trace_id file=$trace_file" >&2
fi

if [[ -n "$S3_URI" ]]; then
  aws s3 sync "$STORE" "$S3_URI" --only-show-errors
fi
