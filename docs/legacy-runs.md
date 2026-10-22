# Legacy runs policy

Legacy runs are kept for audit and history. They are not current
benchmark data.

## Definition

A run is **legacy** when any of these is true:

- `manifest.json` is missing or invalid,
- the manifest has no `model` and the trial output records no model.

This is the rule implemented by `web/private_service.py` and the private
UI. It is deliberately conservative: an unknown value is not promoted to
a current model or lane.

## Why they are separated

Before `run-manifest/v1`, runs had no reliable lane, model, free-only
flag, target version, evidence pointer, or provenance. Showing them next
to current runs would imply comparisons that the data cannot support.
The UI therefore:

- marks them with a `legacy` badge in the Runs and Benchmark tables,
- excludes them from default charts and latest-per-suite cards,
- exposes them only with **Include legacy runs**,
- still allows opening them and reading every stored file.

## What you may do with a legacy run

- Inspect `metadata.json`, `grades.json`, `observations.jsonl`, logs,
  and artifacts.
- Use it to understand history or reproduce an old failure.
- Keep it for audit and S3 retention.
- Optionally backfill a manifest from data already present.

## What you must not do

- Do not compare a legacy run to a current run as if they shared a lane,
  model, suite version, or free-only policy.
- Do not use legacy charts as a current benchmark.
- Do not invent a model, lane, target version, or provenance.
- Do not delete or rewrite the run to make it look current.
- Do not treat a missing judge as zero.

## Backfill

```sh
# Review first; this only uses existing metadata/output.
eval manifest-backfill --store /var/lib/eval-runs --json

# Or non-JSON
eval manifest-backfill --store /var/lib/eval-runs
```

Expected: a count of created/existing/failed sidecars. Backfill does not
guess unknown fields; lanes/models that cannot be derived stay `n/a` and
the run remains legacy for benchmark purposes.

## Inspect a legacy run

```sh
RUN=/var/lib/eval-runs/runs/<suite>/<ref>/run-<run-id>
ls -l "$RUN"
jq '.run_id,.suite,.ref_name,.started_at_ms' "$RUN/metadata.json"
jq '.cases[] | {case_id,pass,trials}' "$RUN/grades.json" 2>/dev/null \
  || jq '.cases[] | {case_id,pass}' "$RUN/metadata.json"
```

In the UI: **Runs** → enable legacy if needed → open the row. The detail
header shows “legacy run” and `n/a` for unknown manifest fields.

## Retention

Legacy runs follow the same store retention as current runs: keep the
local copy until an S3 copy is verified; never rewrite; delete only after
an explicit archive decision. Baselines and decision records stay
forever. See [`run-metrics.md`](run-metrics.md), *Retention*.

## Relationship to the public page

The static public benchmark page selects the latest **passing** run per
ref and does not present the private UI’s legacy toggle. The private UI
is the place to inspect legacy runs. Do not copy legacy numbers into a
public benchmark chart.
