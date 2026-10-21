#!/usr/bin/env bash
# Enable S3 versioning and apply the evidence lifecycle.
# Requires s3:PutBucketVersioning and s3:PutLifecycleConfiguration.
set -euo pipefail

BUCKET="${1:-${EVAL_STORE_BUCKET:-}}"
: "${BUCKET:?provide a bucket or set EVAL_STORE_BUCKET}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIFECYCLE="$ROOT/deploy/private/s3-lifecycle.json"

test -f "$LIFECYCLE" || { echo "missing $LIFECYCLE" >&2; exit 2; }
aws s3api put-bucket-versioning \
  --bucket "$BUCKET" \
  --versioning-configuration Status=Enabled
aws s3api put-bucket-lifecycle-configuration \
  --bucket "$BUCKET" \
  --lifecycle-configuration "file://$LIFECYCLE"
echo "versioning + lifecycle configured for $BUCKET"
