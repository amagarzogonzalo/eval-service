#!/usr/bin/env bash
# Thin wrapper around run_live_service.py (same code path on Linux and Windows).
#
#   run_live_service.sh local    target uvicorn per trial + free-model proxy
#   run_live_service.sh remote   deployed Render service
#
# Tokens are resolved by key_resolver.py from the environment/env files and are
# never printed. Local runs pin every model call to EVAL_FREE_MODEL through
# eval-proxy and reject a non-free served model.
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
PYTHON_BIN="${PYTHON:-python3}"
exec "$PYTHON_BIN" "$ROOT/integrations/research_agents/run_live_service.py" \
  --mode "$MODE" \
  --env-file "${EVAL_ENV_FILE:-$ROOT/.env}" \
  --openrouter-env-file "${EVAL_OPENROUTER_ENV_FILE:-}" \
  --target-repo "${EVAL_RESEARCH_REPO:-}" \
  --target-python "${EVAL_RESEARCH_PYTHON:-}" \
  --store "${EVAL_STORE:-/var/lib/eval-runs}" \
  --otel-endpoint "${EVAL_OTEL_ENDPOINT:-}" \
  --free-model "${EVAL_FREE_MODEL:-nvidia/nemotron-3-ultra-550b-a55b:free}" \
  --ref "${EVAL_REF:-research-service-$MODE}"
