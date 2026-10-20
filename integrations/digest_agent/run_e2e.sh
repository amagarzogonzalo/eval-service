#!/usr/bin/env bash
# Reproducible digest E2E runner.
#
#   run_e2e.sh live     real GitHub + real model via eval-proxy + real PDF
#                       + real SMTP notifier (local test inbox by default)
#   run_e2e.sh replay   recorded GitHub fixture + eval cassette; offline CI
#
# One worker, one lock, bounded proxy budget. Secrets come from the
# environment or SSM (/eval/openrouter_api_key, /eval/github_watch_token)
# and are never printed or written to the run record.
#
# Live (VM, as the eval user or root):
#   bash integrations/digest_agent/run_e2e.sh live
#
# Overrides (all optional):
#   EVAL_BIN, EVAL_STORE, EVAL_S3 (default s3://eval-store-.../eval-runs),
#   EVAL_PROXY_PORT, EVAL_MODEL, EVAL_GITHUB_REPOS, EVAL_LOOKBACK_DAYS,
#   EVAL_MAX_PRS (default 3 for live), EVAL_DIGEST_PYTHON, EVAL_DIGEST_SRC,
#   EVAL_OTEL_ENDPOINT / EVAL_TRACE_OTLP_ENDPOINT, EVAL_RECIPIENT,
#   EVAL_CASSETTE_DIR, EVAL_MAX_REQUESTS, EVAL_MAX_COST_USD,
#   EVAL_PRICE_IN, EVAL_PRICE_OUT, EVAL_S3_SYNC (default 1 live / 0 replay).
set -euo pipefail

MODE="${1:-live}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

EVAL_BIN="${EVAL_BIN:-/srv/eval-service/target/release/eval}"
EVAL_STORE="${EVAL_STORE:-/var/lib/eval-runs}"
EVAL_S3="${EVAL_S3:-s3://eval-store-342940030437-eu-north-1/eval-runs}"
EVAL_DIGEST_PYTHON="${EVAL_DIGEST_PYTHON:-/srv/eval-venvs/digest-agent/bin/python}"
EVAL_DIGEST_SRC="${EVAL_DIGEST_SRC:-/srv/targets/digest_agent/src}"
EVAL_MODEL="${EVAL_MODEL:-openai/gpt-4o-mini}"
EVAL_GITHUB_REPOS="${EVAL_GITHUB_REPOS:-amagarzogonzalo/eval-service}"
EVAL_LOOKBACK_DAYS="${EVAL_LOOKBACK_DAYS:-7}"
EVAL_MAX_PRS="${EVAL_MAX_PRS:-3}"
EVAL_RECIPIENT="${EVAL_RECIPIENT:-digest-e2e@test.invalid}"
EVAL_PROXY_PORT="${EVAL_PROXY_PORT:-8793}"
EVAL_MAX_REQUESTS="${EVAL_MAX_REQUESTS:-200}"
EVAL_MAX_COST_USD="${EVAL_MAX_COST_USD:-0.20}"
EVAL_PRICE_IN="${EVAL_PRICE_IN:-0.15}"
EVAL_PRICE_OUT="${EVAL_PRICE_OUT:-0.60}"
UPSTREAM="${EVAL_UPSTREAM:-https://openrouter.ai/api/v1}"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LABEL="digest-e2e-${MODE}-${STAMP}"
OUT_DIR="${EVAL_OUTPUT_DIR:-${EVAL_STORE}/e2e-artifacts/${LABEL}}"
TRACE_OUT="${EVAL_TRACE_OUT:-${EVAL_STORE}/e2e-traces/${LABEL}.jsonl}"
CASSETTE_DIR="${EVAL_CASSETTE_DIR:-${EVAL_STORE}/cassettes/${LABEL}}"
LOCK_FILE="${EVAL_LOCK_FILE:-${EVAL_STORE}/.digest-e2e.lock}"

log() { printf 'digest-e2e: %s\n' "$*" >&2; }
die() { log "$*"; exit 3; }

fetch_ssm() {
  local name="$1"
  command -v aws >/dev/null 2>&1 || return 0
  aws ssm get-parameter --name "$name" --with-decryption \
    --query Parameter.Value --output text 2>/dev/null || true
}

[[ -x "$EVAL_BIN" ]] || die "eval binary not executable: $EVAL_BIN"
mkdir -p "$EVAL_STORE" "$OUT_DIR" "$(dirname "$TRACE_OUT")"

# One run at a time. flock is released when the process exits.
exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  die "another digest E2E run holds $LOCK_FILE"
fi

PROXY_PID=""
CASSETTE_PID=""
cleanup() {
  [[ -n "$CASSETTE_PID" ]] && kill "$CASSETTE_PID" 2>/dev/null || true
  [[ -n "$PROXY_PID" ]] && kill "$PROXY_PID" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT

wait_for_port() {
  local port="$1"
  for _ in $(seq 1 100); do
    if (exec 3<>"/dev/tcp/127.0.0.1/${port}") 2>/dev/null; then
      exec 3>&- 3<&-
      return 0
    fi
    sleep 0.1
  done
  return 1
}

if [[ "$MODE" == "live" ]]; then
  if [[ -z "${OPENROUTER_API_KEY:-}" ]]; then
    OPENROUTER_API_KEY="$(fetch_ssm /eval/openrouter_api_key)"
    export OPENROUTER_API_KEY
  fi
  if [[ -z "${EVAL_DIGEST_GITHUB_TOKEN:-}" ]]; then
    EVAL_DIGEST_GITHUB_TOKEN="$(fetch_ssm /eval/github_watch_token)"
    export EVAL_DIGEST_GITHUB_TOKEN
  fi
  [[ -n "${OPENROUTER_API_KEY:-}" ]] || die "OPENROUTER_API_KEY missing (env or SSM)"
  [[ -n "${EVAL_DIGEST_GITHUB_TOKEN:-}" ]] || die "EVAL_DIGEST_GITHUB_TOKEN missing (env or SSM)"

  log "starting eval-proxy on 127.0.0.1:${EVAL_PROXY_PORT} (cap ${EVAL_MAX_REQUESTS} req / \$${EVAL_MAX_COST_USD})"
  "$EVAL_BIN" proxy --bind 127.0.0.1 --port "$EVAL_PROXY_PORT" \
    --key-env OPENROUTER_API_KEY --provider openrouter \
    --project digest-agent --tag digest-e2e-live --run-id "$LABEL" \
    --store "$EVAL_STORE" \
    --max-requests "$EVAL_MAX_REQUESTS" \
    --max-cost-usd "$EVAL_MAX_COST_USD" \
    --price-in-per-1m "$EVAL_PRICE_IN" \
    --price-out-per-1m "$EVAL_PRICE_OUT" \
    --upstream-timeout-s 180 \
    --cassette-dir "$CASSETTE_DIR" \
    "$UPSTREAM" &
  PROXY_PID=$!
  wait_for_port "$EVAL_PROXY_PORT" || die "eval-proxy did not start"

  export EVAL_DIGEST_LIVE=1
  export EVAL_DIGEST_API_KEY="dummy-eval-proxy-observes"
  export EVAL_DIGEST_ENDPOINT="http://127.0.0.1:${EVAL_PROXY_PORT}/v1"
  export EVAL_DIGEST_MODEL="$EVAL_MODEL"
  export EVAL_DIGEST_TIMEOUT_S="${EVAL_DIGEST_TIMEOUT_S:-120}"
  export EVAL_DIGEST_MAX_REQUESTS="$EVAL_MAX_REQUESTS"
  export EVAL_DIGEST_GITHUB_REPOS="$EVAL_GITHUB_REPOS"
  export EVAL_DIGEST_LOOKBACK_DAYS="$EVAL_LOOKBACK_DAYS"
  export EVAL_DIGEST_MAX_PRS="$EVAL_MAX_PRS"
  export EVAL_DIGEST_RECIPIENT="$EVAL_RECIPIENT"
  export EVAL_S3_SYNC="${EVAL_S3_SYNC:-1}"
  export EVAL_DIGEST_OUTPUT_DIR="$OUT_DIR"
  export EVAL_DIGEST_SMTP_INBOX="$OUT_DIR/mailbox"
  export EVAL_TRACE_OUT="$TRACE_OUT"
  export EVAL_TRACE_RUN_ID="$LABEL"
  SUITE="integrations/digest_agent/live_e2e.yaml"
  ORIGIN="live"
  REF="${EVAL_REF:-live}"
else
  [[ "$MODE" == "replay" ]] || die "usage: run_e2e.sh live|replay"
  REPLAY_MODE="${EVAL_REPLAY_MODE:-fixture}"
  export EVAL_DIGEST_LIVE=1
  export EVAL_DIGEST_API_KEY="dummy-eval-proxy-observes"
  export EVAL_DIGEST_MODEL="$EVAL_MODEL"
  # Replay still executes the full pipeline; the per-process budget must
  # be larger than the recorded call count.
  export EVAL_DIGEST_MAX_REQUESTS="${EVAL_REPLAY_MAX_REQUESTS:-200}"
  export EVAL_DIGEST_MAX_PRS=0
  export EVAL_DIGEST_E2E_FIXTURE="${ROOT}/datasets/digest_e2e/github_activity.json"
  export EVAL_DIGEST_RECIPIENT="$EVAL_RECIPIENT"
  export EVAL_S3_SYNC="${EVAL_S3_SYNC:-0}"
  export EVAL_DIGEST_OUTPUT_DIR="$OUT_DIR"
  export EVAL_DIGEST_SMTP_INBOX="$OUT_DIR/mailbox"
  export EVAL_TRACE_OUT="$TRACE_OUT"
  export EVAL_TRACE_RUN_ID="$LABEL"
  if [[ "$REPLAY_MODE" == "cassette" ]]; then
    CASSETTE_DIR="${EVAL_CASSETTE_DIR:-${ROOT}/datasets/digest_e2e/cassette}"
    [[ -d "$CASSETTE_DIR" ]] || die "cassette directory missing: $CASSETTE_DIR"
    export EVAL_DIGEST_ENDPOINT="http://127.0.0.1:8788/v1"
    unset EVAL_DIGEST_MOCK_FILE
    log "starting cassette replay on 127.0.0.1:8788"
    "$EVAL_BIN" cassette "$CASSETTE_DIR" --project digest-agent \
      --store "$EVAL_STORE" --tag digest-e2e-replay --run-id "$LABEL" &
    CASSETTE_PID=$!
    wait_for_port 8788 || die "cassette replay did not start"
  elif [[ "$REPLAY_MODE" == "fixture" ]]; then
    # Synthetic LLM fixture: deterministic and offline. It is labeled as
    # a fixture in docs and is never live evidence.
    export EVAL_DIGEST_ENDPOINT="http://127.0.0.1:9/v1"
    export EVAL_DIGEST_MOCK_FILE="${EVAL_DIGEST_MOCK_FILE:-${ROOT}/datasets/digest_e2e/llm_fixture.json}"
    [[ -f "$EVAL_DIGEST_MOCK_FILE" ]] || die "LLM fixture missing: $EVAL_DIGEST_MOCK_FILE"
  else
    die "EVAL_REPLAY_MODE must be fixture or cassette"
  fi
  SUITE="integrations/digest_agent/live_e2e_replay.yaml"
  ORIGIN="replay"
  REF="${EVAL_REF:-replay}"
fi

export EVAL_DIGEST_PYTHON EVAL_DIGEST_SRC
export EVAL_TRACE_SERVICE="${EVAL_TRACE_SERVICE:-digest-agent}"
if [[ -n "${EVAL_TRACE_OTLP_ENDPOINT:-}" ]]; then
  export EVAL_TRACE_OTLP_ENDPOINT
fi
# Agent A's trace spine convention (docs/observability.md).
export EVAL_OTEL_ENDPOINT="${EVAL_OTEL_ENDPOINT:-http://127.0.0.1:4328}"

log "running $SUITE (origin=$ORIGIN, ref=$REF, workers=1)"
set +e
"$EVAL_BIN" run "$SUITE" --ref "$REF" --origin "$ORIGIN" --workers 1 \
  --store "$EVAL_STORE" --json >"$OUT_DIR/run.json"
STATUS=$?
set -e

RUN_ID="$(python3 - "$OUT_DIR/run.json" <<'PY' 2>/dev/null || true
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("run_id", ""))
except Exception:
    print("")
PY
)"
TRACE_ID="$(python3 - "$OUT_DIR/run.json" <<'PY' 2>/dev/null || true
import json, sys
try:
    data = json.load(open(sys.argv[1]))
    for case in data.get("cases", []):
        for trial in case.get("trials", []):
            envelope = trial.get("output") or {}
            inner = envelope.get("output") or envelope
            trace_id = (inner.get("data") or {}).get("trace_id")
            if trace_id:
                print(trace_id)
                raise SystemExit
except Exception:
    pass
print("")
PY
)"

log "status=$STATUS run_id=${RUN_ID:-?} trace_id=${TRACE_ID:-?}"
log "artifacts=$OUT_DIR traces=$TRACE_OUT"
log "cassettes=${CASSETTE_DIR}"

if [[ "${EVAL_S3_SYNC:-0}" == "1" ]]; then
  if command -v aws >/dev/null 2>&1; then
    log "syncing store to $EVAL_S3"
    timeout 240 aws s3 sync "$EVAL_STORE" "$EVAL_S3" --only-show-errors \
      || log "S3 sync failed"
  else
    log "aws CLI missing; skipping S3 sync"
  fi
fi

exit "$STATUS"

