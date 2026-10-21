#!/usr/bin/env bash
# Free-only digest entrypoint E2E runner.
#
#   run_e2e.sh live     real entrypoint + real GitHub + free Gemini/Groq models
#                       + real PDF + real SMTP test inbox
#   run_e2e.sh replay   real entrypoint + recorded GitHub/LLM fixtures,
#                       real PDF + real SMTP test inbox, no network
#
# The entrypoint is `python -m digest_agent.main` in a temporary working
# directory. Only the selected Gemini or Groq key reaches it; OpenRouter,
# Mistral, and the other provider key are removed. No paid fallback.
#
# One worker, one lock, S3 sync on live, no live run in PR CI.
#
#   EVAL_DIGEST_GITHUB_TOKEN=<read-only token or SSM> \
#     bash integrations/digest_agent/run_e2e.sh live
#
# Overrides: EVAL_BIN, EVAL_STORE, EVAL_S3, EVAL_DIGEST_PYTHON,
# EVAL_DIGEST_SRC, EVAL_DIGEST_GITHUB_REPOS, EVAL_LOOKBACK_DAYS,
# EVAL_RECIPIENT, EVAL_OTEL_ENDPOINT, EVAL_TRACE_OUT,
# EVAL_DIGEST_OUTPUT_DIR, EVAL_S3_SYNC.
set -euo pipefail

MODE="${1:-live}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

EVAL_BIN="${EVAL_BIN:-/srv/eval-service/target/release/eval}"
EVAL_STORE="${EVAL_STORE:-/var/lib/eval-runs}"
EVAL_S3="${EVAL_S3:-s3://eval-store-342940030437-eu-north-1/eval-runs}"
EVAL_DIGEST_PYTHON="${EVAL_DIGEST_PYTHON:-/srv/eval-venvs/digest-agent/bin/python}"
EVAL_DIGEST_SRC="${EVAL_DIGEST_SRC:-/srv/targets/digest_agent/src}"
EVAL_GITHUB_REPOS="${EVAL_GITHUB_REPOS:-amagarzogonzalo/eval-service}"
EVAL_LOOKBACK_DAYS="${EVAL_LOOKBACK_DAYS:-7}"
EVAL_RECIPIENT="${EVAL_RECIPIENT:-digest-e2e@test.invalid}"

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LABEL="digest-e2e-${MODE}-${STAMP}"
OUT_DIR="${EVAL_DIGEST_OUTPUT_DIR:-${EVAL_STORE}/e2e-artifacts/${LABEL}}"
TRACE_OUT="${EVAL_TRACE_OUT:-${EVAL_STORE}/e2e-traces/${LABEL}.jsonl}"
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

exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  die "another digest E2E run holds $LOCK_FILE"
fi

export EVAL_DIGEST_PYTHON EVAL_DIGEST_SRC
export EVAL_DIGEST_GITHUB_REPOS="$EVAL_GITHUB_REPOS"
export EVAL_DIGEST_LOOKBACK_DAYS="$EVAL_LOOKBACK_DAYS"
export EVAL_DIGEST_RECIPIENT="$EVAL_RECIPIENT"
export EVAL_DIGEST_OUTPUT_DIR="$OUT_DIR"
export EVAL_DIGEST_SMTP_INBOX="$OUT_DIR/mailbox"
export EVAL_TRACE_OUT="$TRACE_OUT"
export EVAL_TRACE_RUN_ID="$LABEL"
export EVAL_TRACE_SERVICE="${EVAL_TRACE_SERVICE:-digest-agent}"
export EVAL_OTEL_ENDPOINT="${EVAL_OTEL_ENDPOINT:-http://127.0.0.1:4328}"
export EVAL_DIGEST_PROVIDER="${EVAL_DIGEST_PROVIDER:-gemini}"
export EVAL_DIGEST_ALLOW_PAID=0

if [[ "$MODE" == "live" ]]; then
  export EVAL_DIGEST_GEMINI_API_KEY="${EVAL_DIGEST_GEMINI_API_KEY:-${GEMINI_API_KEY:-}}"
  export EVAL_DIGEST_GROQ_API_KEY="${EVAL_DIGEST_GROQ_API_KEY:-${GROQ_API_KEY:-}}"
  # Fallback: read only the selected provider key from the target's own
  # dotenv file. Other lines (including other provider keys) are never
  # imported.
  export EVAL_DIGEST_PROVIDER_KEY_FILE="${EVAL_DIGEST_PROVIDER_KEY_FILE:-/srv/targets/digest_agent/.env}"
  if [[ "$EVAL_DIGEST_PROVIDER" == "gemini" ]]; then
    provider_key="$EVAL_DIGEST_GEMINI_API_KEY"
  else
    provider_key="$EVAL_DIGEST_GROQ_API_KEY"
  fi
  if [[ -z "$provider_key" && ! -r "$EVAL_DIGEST_PROVIDER_KEY_FILE" ]]; then
    die "no free-tier provider key (env or EVAL_DIGEST_PROVIDER_KEY_FILE); no paid fallback"
  fi
  if [[ -z "${EVAL_DIGEST_GITHUB_TOKEN:-}" ]]; then
    EVAL_DIGEST_GITHUB_TOKEN="$(fetch_ssm /eval/github_watch_token)"
    export EVAL_DIGEST_GITHUB_TOKEN
  fi
  [[ -n "${EVAL_DIGEST_GITHUB_TOKEN:-}" ]] \
    || die "EVAL_DIGEST_GITHUB_TOKEN missing (env or SSM)"
  export EVAL_S3_SYNC="${EVAL_S3_SYNC:-1}"
  SUITE="integrations/digest_agent/live_e2e.yaml"
  ORIGIN="live"
  REF="${EVAL_REF:-live}"
elif [[ "$MODE" == "replay" ]]; then
  export EVAL_DIGEST_E2E_FIXTURE="${ROOT}/datasets/digest_e2e/github_activity.json"
  export EVAL_DIGEST_LLM_FIXTURE="${ROOT}/datasets/digest_e2e/llm_fixture.json"
  export EVAL_S3_SYNC="${EVAL_S3_SYNC:-0}"
  SUITE="integrations/digest_agent/live_e2e_replay.yaml"
  ORIGIN="replay"
  REF="${EVAL_REF:-replay}"
else
  die "usage: run_e2e.sh live|replay"
fi

log "running $SUITE (origin=$ORIGIN, ref=$REF, workers=1, provider=$EVAL_DIGEST_PROVIDER/free-only)"
set +e
"$EVAL_BIN" run "$SUITE" --ref "$REF" --origin "$ORIGIN" --workers 1 \
  --store "$EVAL_STORE" --lane batch \
  --model "${EVAL_DIGEST_PROVIDER:-gemini}:free" \
  --target-version "${EVAL_DIGEST_TARGET_VERSION:-digest-agent}" \
  --free-only --json >"$OUT_DIR/run.json"
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
log "artifacts=$OUT_DIR metrics=$OUT_DIR/metrics-<case>.json traces=$TRACE_OUT"

if [[ "${EVAL_S3_SYNC:-0}" == "1" ]]; then
  if command -v aws >/dev/null 2>&1; then
    log "syncing store to $EVAL_S3"
    timeout 240 aws s3 sync "$EVAL_STORE" "$EVAL_S3" --only-show-errors \
      --exclude '*/mailbox/certs/*' \
      || log "S3 sync failed"
  else
    log "aws CLI missing; skipping S3 sync"
  fi
fi

exit "$STATUS"
