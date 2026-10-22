# Run manifest, metrics, and repeatability

Status: `run-manifest/v1` and `run-metrics/v1` are defined here and in
`eval-core::run_manifest`. CLI runs write the sidecar, legacy stores can
be backfilled, the dashboard reads it, and the JSON Schema is emitted by
`eval schema`. Nothing in this document changes a target repository or a
deployed service.

## 1. Audit of the current store and reports

### Store layout

```text
<store>/runs/<suite>/<ref>/run-<run_id>/metadata.json
                                          observations.jsonl
                                          grades.json
                                          decision.json        (optional)
                                          manifest.json        (new, optional)
<store>/baselines/<suite>.json
<store>/usage/<project>/events-*.jsonl
<store>/usage/<project>/tags.json
```

`metadata.json` is a `RunRecord`: `schema_version`, `run_id`, `suite`,
`suite_version`, `code_version`, `ref_name`, `started_at_ms`, `target`,
`files_root`, `trials_total`, `cases[]`, optional `gate`. Each case
carries trials, grades, `pass`, and a stable digest.
`observations.jsonl` is the append-only raw evidence; `grades.json` is
the derived view; `decision.json` is the gate decision.

`RunStore::save` redacts the record before hashing, then derives
`run_id = <started_at_ms>-<blake3(canonical_record)[:8]>`. The record's
own `run_id` is cleared before hashing, so re-saving a loaded record is
idempotent. New run directories are staged and renamed into place;
`metadata.json`, `observations.jsonl`, `grades.json`, `decision.json`,
and `manifest.json` are write-once. Different bytes for an existing run
id are rejected instead of overwritten.

### Report code

- `eval-graders::portfolio` reads the canonical stores through
  `RunStore`; there is no shadow database.
- `eval-graders::dashboard` renders one self-contained HTML file. It
  already shows pass rate, p50/p95 wall latency, legacy token/cost
  coverage, failed cases, regression state, history, usage, and file
  links. Corrupt records are shown as unreadable, not dropped.
- `eval-cli` exposes this as `eval projects` and `eval dashboard`.
- `web/build_benchmarks.py` renders the public benchmark page from the
  canonical run store. The private UI (`web/private_service.py` plus
  `web/private_ui/`) reads the same store directly and is the
  authoritative current/legacy evidence view. See `docs/production.md`,
  `docs/benchmark-evidence.md`, and `docs/legacy-runs.md`.

### Gaps against the required schema

| Required datum | Current state |
|---|---|
| suite, ref, target, eval version | suite/ref present; eval version only as `code_version`; target version absent |
| lane (batch/local-service/remote-service/replay) | absent |
| model, free-only | only legacy `ai.model` inside trial output; no run-level model or free flag |
| trials and case outcomes | present and derivable |
| failure taxonomy | raw statuses present; no one-class-per-failed-case summary |
| p50/p95 latency | present for wall time; no service cold start/queue wait |
| observed tokens/cost | legacy `ai` sums only, with coverage count; `llm/v1` events live in a separate store |
| service availability | absent |
| artifact hashes | paths only; no hashes |
| trace/task ids | only inside `llm/v1` events, not at run level |
| S3 paths | absent; sync is bucket-wide and unrecorded |
| budgets | target budget and case timeouts exist in records/suites; no run-level budget record |
| provenance | minimal (`code_version` and `files_root`) |

The implementation closes the identity/metrics gaps: CLI runs now write
lane, model, free-only, budgets, outcomes, taxonomy, usage, and
provenance; legacy stores can be backfilled. Service exporters,
artifact hashing, and S3 pointer automation remain follow-up work.

### Other findings

1. Dashboard totals intentionally count the **newest run per suite**,
   so baseline/work histories never double-count. Keep this rule.
2. Regression comparison previously compared any baseline to any
   candidate. A lane/model/free-only/suite-version change could
   therefore produce a regression claim that is not like-for-like.
3. Timeouts are handled negative outcomes for exit-code purposes
   (exit 1), while crash/protocol errors are infrastructure. The
   manifest keeps them as distinct failure classes; `outcomes.infra`
   remains crash/protocol only so the existing suite exit contract does
   not change.
4. History is capped at five runs per ref and twenty per project. The
   new trend view reads the last 30 runs per suite; deeper time-series
   queries remain follow-up work.
5. The S3 sync is append-only (`aws s3 sync` without `--delete`) and
   the bucket has versioning enabled. There is no retention job yet;
   the only lifecycle rule aborts incomplete multipart uploads.

## 2. `run-manifest/v1` contract

Canonical Rust types: `eval_core::run_manifest`. Canonical location:
`<run_dir>/manifest.json`. The sidecar is optional for old runs. A
present but invalid sidecar is reported as `manifest_error`, never
silently ignored.

### Identity and configuration

| Field | Type | Required | Notes |
|---|---|---|---|
| `manifest_version` | integer | yes | exactly `1` |
| `run_id` | string | yes | immutable, equal to the directory suffix |
| `suite.id`, `suite.version` | string, integer | yes | must match `metadata.json` |
| `suite.path` | string or null | no | suite file at run time |
| `target.name`, `target.version` | string or null | no | target project identity |
| `target.mode` | `worker`/`oneshot` or null | no | protocol mode |
| `lane` | enum or null | yes for new runs | `batch`, `local-service`, `remote-service`, `replay` |
| `eval_version` | string or null | yes for new runs | platform version/commit |
| `ref_name` | string | yes | `baseline`, `work`, `live-*`, `replay`, ... |
| `model.provider`, `model.model` | string or null, string | no | model under test, when applicable |
| `free_only` | boolean or null | yes for new runs | true = only free-tier/free models were eligible |

`lane`, `eval_version`, and `free_only` are nullable so a legacy run can
be described honestly as unknown rather than guessed. New manifests
must set them; `n/a` is a valid display, never a claim.

### Timestamps and budgets

| Field | Type | Notes |
|---|---|---|
| `timestamps.started_at_ms` | integer | required |
| `timestamps.finished_at_ms` | integer or null | `n/a` for legacy |
| `timestamps.duration_ms` | integer or null | measured, not derived when absent |
| `budgets.target_budget_ms` | integer or null | target-declared budget |
| `budgets.max_case_timeout_ms` | integer or null | largest configured case timeout |
| `budgets.max_output_bytes` | integer or null | output cap |
| `budgets.cost_usd_cap` | number or null | planning cap, not spend |
| `budgets.tokens_cap` | integer or null | planning cap |

Budgets describe allowances. They are never used as observed spend.

### Metrics

```jsonc
{
  "metrics_version": 1,
  "trials": {"planned": 10, "completed": 10, "usage_reporting": 4},
  "outcomes": {
    "total": 5, "passed": 4, "failed": 1,
    "infra": 0, "timeouts": 0, "skipped": null
  },
  "failure_classes": {"grader": 1},
  "latency": {"n": 10, "min_ms": 81, "p50_ms": 140, "p95_ms": 415, "max_ms": 415},
  "usage": {
    "model": {"provider": "groq", "model": "llama-3.1-8b-instant"},
    "tokens_in": 1200, "tokens_out": 300,
    "cached_tokens": null, "reasoning_tokens": null,
    "cost_usd": 0.00042, "cost_provenance": "reported",
    "pricing_version": null,
    "reporting_trials": 4, "total_trials": 10
  },
  "quality": {"grader": "judge_lite", "mean_score": 0.83, "n": 5},
  "stability": {"passed": 1, "total": 1},
  "service": {
    "availability": {"attempts": 100, "successes": 98, "ratio": null},
    "cold_start": {"n": 2, "min_ms": 90, "p50_ms": 120, "p95_ms": 180, "max_ms": 180},
    "queue_wait": {"n": 2, "min_ms": 5, "p50_ms": 10, "p95_ms": 20, "max_ms": 20}
  }
}
```

Rules:

- `null` means **n/a**: not observed, not applicable, or not exposed by
  the lane. It is never rendered as `0` and never treated as `0` in a
  comparison.
- Zero is written only when a measurement is genuinely zero (for
  example `outcomes.failed: 0` or a provider-reported `cost_usd: 0.0`).
- `usage.reporting_trials` / `usage.total_trials` is the coverage
  denominator. A token sum without coverage must not be presented as a
  complete run total.
- `cost_provenance` is `reported` or `estimated`. Estimated costs carry
  the `pricing_version` used; reported costs do not.
- Latency samples are runner-measured wall milliseconds. Percentiles
  are nearest-rank with `n > 0`; empty samples are `null`.
- `quality` is the mean `judge_lite` score over scored cases. It is not
  a model-judge claim.
- `stability` counts explicit `stability` grader results. It is not
  inferred from repeated runs without that grader.
- `service` is only meaningful on service lanes. Batch runs leave it
  `null` (n/a), not "100% available".

### Evidence and provenance

| Field | Type | Notes |
|---|---|---|
| `evidence.trace_id`, `evidence.task_id` | string or null | join keys to traces/tasks |
| `evidence.trace_url` | string or null | optional external link |
| `evidence.artifacts[]` | array | `path`, optional `kind`, `bytes`, `hash_alg`, `hash` |
| `evidence.s3.bucket`, `run_prefix` | string or null | object-store location |
| `evidence.s3.trace_key`, `artifact_prefix` | string or null | narrower pointers |
| `provenance.origin` | `live`/`replay`/`synthetic`/`unknown` | replay never reads as live |
| `provenance.eval_git_commit`, `eval_git_dirty` | string/bool or null | platform code |
| `provenance.target_git_commit` | string or null | target under test |
| `provenance.host`, `runner`, `recorded_by` | string or null | execution context |
| `provenance.record_source`, `notes` | string or null | free-form, redacted |

Artifact hashes are optional; when present, `hash_alg` is required. A
missing hash is n/a, not a zero-length file.

### Failure taxonomy

One primary class per failed case, in this precedence order:

1. `protocol_error`
2. `crash`
3. `timeout`
4. `case_error`
5. `grader` (trials ran, a grader failed)
6. `service_unavailable`, `budget_exceeded`, `missing_artifact`,
   `cancelled` (lane/integration supplied)
7. `unknown` (cannot classify without guessing)

Invariant: `sum(failure_classes) == outcomes.failed` for a manifest with
complete outcomes. `outcomes.infra` is the crash/protocol subset and
deliberately excludes timeouts (timeouts remain graded failures, exit 1).
`outcomes.timeouts` counts failed cases with at least one timeout
observation; it can overlap `infra` when a case observed both a timeout
and a crash. Use the primary failure class for ranking and these subset
counts for diagnosis.

### Versioning

- Additive fields are minor within v1 and readers ignore unknown fields.
- Removing or retyping a field requires a new manifest version and a
  forward-only converter; old run directories are never rewritten.
- A reader that sees a newer version refuses to interpret it rather than
  guessing.
- `metrics_version` versions the measurement block independently of
  run identity.
- `eval schema` exposes `run_manifest_schema` (JSON Schema 2020-12) and
  `run_manifest_keys`. A test serializes a real manifest and asserts the
  schema covers every emitted field, so the two cannot drift silently.

## 3. Dashboard design

The dashboard remains one static HTML file generated from
`PortfolioReport`. No server, database, or JS dependency is added.

### Views and definitions

1. **Portfolio totals** — projects, suites with runs, case-weighted pass
   rate, passed/failed/infra counts. Definition text stays on the page:
   cases are counted from the newest run per suite, not every ref.
2. **Project overview** — status, latest run, cases, regression state,
   coverage, last success/failure. Unavailable projects remain visible.
3. **Per-suite drill-down** — newest run per ref, history (capped),
   failed cases with reasons, baseline state.
4. **Run table** — ref, lane, model, free-only flag, run id, code
   version, result, quality (mean judge_lite + n), stability, cases,
   p50/p95 wall latency, tokens/cost with reporting coverage, service
   availability/cold start/queue wait, failure classes, and evidence
   links. When a manifest carries usage, it is preferred over legacy
   `ai` totals and keeps reported/estimated provenance.
5. **Trend (last 30 runs)** — per suite, newest first: ref, lane,
   result/rate, cases, p50/p95, tokens in/out, and cost. This is the
   repeatability view; the short history list remains separate.
6. **Quality vs latency** — one row per suite/ref with a measured
   quality or latency value. Missing quality or latency is `n/a`; no
   point is invented.
7. **Cost/tokens where observed** — every row states `n/a (0/N trials
   reported usage)` when nothing was reported. Reported and estimated
   cost remain distinguishable in usage views.
8. **Stability** — explicit `stability` grader pass ratio; `n/a` when
   the suite has no stability grader.
9. **Failure classes** — per-run primary-class counts, aggregated from
   the taxonomy above. Zero failures renders `none`, not `n/a`.
10. **Regressions vs baseline** — only when a baseline pointer exists
   and the candidate is comparable (same suite version, and matching
   lane/free-only/model/eval version when both manifests know them).
   Otherwise the page says `not compared: <reason>` and makes no claim.
11. **Evidence links** — local metadata/observations/grades files, trace
    id or trace URL, task id, S3 bucket/prefix, artifact count.
12. **Filters** — project, status, suite, lane, and since-date. Filtered
    rows are hidden, not deleted; a live count states what is shown.
    Usage rows are exempt from outcome filters by design.

### Honesty rules

- Never zero-fill tokens, cost, latency, quality, availability, or
  service timings.
- Never mix production, benchmark, and dev usage.
- Never compare different lanes, models, suite versions, or free-only
  policies as if they were the same measurement.
- Show corrupt records and corrupt manifests as errors with context.
- Mark synthetic/replay provenance wherever a measurement is displayed.

## 4. Immutable run ids, retention, compare semantics

### Run ids

- Format: `<started_at_ms>-<hash8>`, where `hash8` is the first eight
  hex characters of BLAKE3 over the redacted canonical record.
- A run directory is immutable. New directories are written in a
  staging directory and renamed into place. Re-saving identical content
  is idempotent; different bytes for an existing run id are rejected,
  including modified observations, grades, or manifests.
- `manifest.json` follows the same rule: identical rewrites are
  idempotent, different content is rejected.
- Baselines are pointers, never copies: `baselines/<suite>.json` names
  `suite`, `ref_name`, and `run_id`.

### Retention

Recommended policy (documented now, not applied to AWS in this change):

| Tier | Keep | Notes |
|---|---|---|
| Baselines + decision records | forever | pointers are tiny; deleting them destroys comparability |
| Newest N runs per suite/ref (N >= 20) | forever | dashboard/trend working set |
| Other local run directories | 180 days | archive to S3 first; never delete local-only evidence |
| S3 object versions | 365 days | bucket versioning already enabled |
| Abandoned multipart uploads | 7 days | existing lifecycle rule |
| Secrets | never stored | redaction runs before persistence |

Rules: restore before expiry; delete only after verifying the S3 copy;
never rewrite an existing run; retention jobs are explicit and logged.
No retention job is enabled by this change.

### Compare semantics

- `eval compare` uses the explicit baseline pointer and the chosen
  candidate (normally the newest run of the ref). It never picks a new
  baseline silently.
- Gate exit codes remain: `0` pass, `1` regression/failure, `2` infra.
- Deterministic checks run first (status flips, grade flips,
  deterministic digest changes, removed cases), then the pass-rate
  floor and the seeded paired-bootstrap diagnostic with written
  uncertainty.
- Like-for-like precondition: same suite id and version. When both
  manifests exist, also require the same lane, free-only flag, model,
  and eval version. Unknown values do not block a comparison, but they
  are displayed as n/a; differing known values block the regression
  claim.
- Both `eval compare` and the dashboard enforce this precondition.
  `eval compare` exits 2 with `runs are not like-for-like: <reason>`
  and writes no decision; it never reports that as a pass.
- A blocked comparison is not a pass. It is "not compared", rendered
  explicitly in the dashboard and never smuggled into the pass rate.
- Re-grading stored observations recomputes grades without re-executing
  the target; it does not change the run id or the raw evidence.

## 5. Minimal changes for trends and baseline comparison

Implemented:

1. `eval-core::run_manifest` — versioned `RunManifest` / `RunMetrics`
   types, n/a-preserving serialization, validation, one-primary-class
   failure taxonomy, timeout subset, JSON Schema, manifest load/write.
2. `RunStore` — staged run directories, write-once evidence, stable
   content-addressed ids, `manifest.json` load/save, and idempotent
   `backfill_manifests`.
3. CLI producers — `eval run` and `eval onboard` write manifests;
   `run` accepts `--lane`, `--model`, `--target-version`,
   `--free-only`/`--paid`. `eval manifest-backfill` fills legacy stores
   without guessing lane or provenance.
4. CLI compare — enforces the same like-for-like precondition as the
   dashboard and exits 2 when runs are not comparable.
5. `PortfolioReport` — one metric derivation path (`RunMetrics`),
   manifest attachment/errors, failure classes, quality, stability,
   per-suite trend points, and like-for-like regression gating.
6. Dashboard — lane/model/free-only, manifest-first usage with cost
   provenance, quality/stability, service timings, failure classes,
   trace/task/S3/artifact evidence, lane filter, quality-vs-latency,
   and a last-30-runs trend view.
7. `eval schema` — advertises versions, lanes, failure classes, field
   paths, and the JSON Schema.
8. Offline tests — schema/n-a semantics, run/manifest immutability,
   backfill idempotence, CLI manifest writing and cross-lane refusal,
   trend ordering, dashboard totals against an independent store scan,
   and manifest/evidence rendering.

Remaining follow-up:

| Phase | Change | Risk | Acceptance |
|---|---|---|---|
| A | Service lane exporters (availability, cold start, queue wait) | medium | remote metrics are n/a unless actually exposed |
| B | Artifact hashing/upload automation and S3 pointer recording | medium | hashes match bytes; upload verified before pointer |
| C | Retention/archive job (local -> S3, verified copy first) | high | dry-run report, no deletion of baselines/latest N |
| D | External benchmark page consumes `eval projects --json` | low | same totals as dashboard; provenance labels preserved |

Do not merge without review. The remaining phases touch deployed
services, artifacts, or infrastructure; this change stays within the
engine, store, CLI, dashboard, and tests.

## 6. Offline verification

```sh
cargo test -p eval-core --test run_manifest_tests --test core_tests
cargo test -p eval-graders --test dashboard_tests --test portfolio_tests --test manifest_tests
cargo test -p eval-cli --test cli_tests
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The dashboard tests build temporary on-disk stores, save real run
records, attach manifests, render HTML, and cross-check totals against
an independent scan of `metadata.json`. No live evaluation, network
call, AWS action, or target repository edit is involved.
