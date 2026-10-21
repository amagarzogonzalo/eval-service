#!/usr/bin/env bash
# Restore the S3 mirror into a temporary directory and validate it.
# Never overwrite the live store during a drill.
set -euo pipefail

: "${STORES3:?set STORES3=s3://bucket/prefix}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$(mktemp -d "${TMPDIR:-/tmp}/eval-restore.XXXXXX")"
trap 'rm -rf "$DEST"' EXIT

echo "restoring $STORES3 -> $DEST"
aws s3 sync "$STORES3" "$DEST" --only-show-errors
python3 "$ROOT/scripts/verify_store.py" "$DEST"
echo "restore drill OK"
