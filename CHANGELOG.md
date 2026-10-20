# Changelog

## Unreleased — control plane and judge calibration (Phase 5/7)

- `eval watch`: read-only GitHub issue polling (no comments/statuses),
  allowlisted suites, body-hash debounce, restart-safe queue, atomic
  state with backup, per-state-directory lock, request ids stored in run
  records for crash-safe no-duplicate recovery, heartbeat with pending/
  running/failed counts and a stale-running alarm, and optional dashboard
  refresh. Read-only token comes from the environment, never state/logs.
- `eval calibrate`: offline human-vs-judge calibration over JSONL:
  Cohen's kappa with deterministic bootstrap CI, A/B position-swap and
  first-position metrics, verbosity diagnostics, rubric-threshold sweep,
  content-addressed versioned JSON report + Markdown, and an
  advisory/gating decision pinned to the H4 kappa bar (lower CI >= 0.6,
  n >= 30, position consistency >= 0.8). `--require-gating` makes the bar
  a hard CI gate.
- `RunRecord` gains an additive optional `request_id`; old records load
  unchanged. `RunStore::find_request_id` supports recovery/dedupe.
- New docs: `docs/watch.md`, `docs/judge-calibration.md`; synthetic
  sample dataset `datasets/judge_calibration/digest_v1.jsonl`.

## Unreleased — usage platform vertical slice

- New crate `eval-usage`: versioned `llm/v1` event contract (identity,
  request/attempt/retry semantics, streaming/TTFT, token categories,
  outcome taxonomy, cost provenance, prompt ids, task/trace/run/case
  linkage, required `context`: production|benchmark|dev, `origin`
  live/replay/synthetic), validation, idempotent JSONL import
  (per-line rejections, project binding, tag/run-id stamps, tag→run
  registry), crash-safe dedupe (index ∪ log scan), versioned pricing
  table with exact→provider→global resolution, aggregation (requests
  counted once, attempts always counted; per-model/task/daily views;
  OptSum absent-instead-of-zero everywhere).
- `eval usage-import`, `eval usage-report` (with `--context`),
  `eval collect` (loopback-only, 1 MiB cap, rate cap, optional
  bearer token, constant-time compare).
- Portfolio `attach_usage` + dashboard usage/benchmark-comparison
  views with provenance, pricing-version, and synthetic labels.
- Precedence rule: usage views aggregate events only; legacy `AiUsage`
  blocks flagged, never summed (tested).
- Integrations emit real usage: digest_agent port decorator (latency/
  outcome/errors measured, tokens honestly absent) and llm_calls live
  path (provider `usage` blocks → events + ai blocks), with mock
  transport tests and worker-spanning request budgets (tested).
- Fixture benchmark (summarizer-a vs -b): quality (mean judge_lite
  score) vs. estimated cost vs. measured latency; labeled synthetic.
- Fixed along the way: concurrent JSONL append loss on Windows
  (region-locked writer; buffered and O_APPEND appends both lose),
  msvcrt unlock-offset quirk, grade scores preserving raw signal,
  `usage-import --ref` flag consistency.
- MSRV 1.85 verified by compiling and testing on the 1.85 toolchain
  (let-chains removed where the minimum requires it).

## 0.1.0 (2026-10-10) — MVP + Public Beta preparation

Initial release. Rust evaluation platform: one engine that evaluates
heterogeneous projects through a small subprocess protocol, detects
meaningful regressions, and explains its decisions.

### Engine

- **eval-protocol**: versioned `eval/v1` subprocess wire contract with
  worker and oneshot modes, strict-target/tolerant-runner parsing, 8 MiB
  line cap, gapless seq tracking, and a committed, drift-checked JSON Schema.
- **eval-core**: suite config (YAML), run records with raw/derived separation
  (`observations.jsonl` + `grades.json`), file store with blake3 content-addressed
  run IDs, secret redaction with property tests.
- **eval-runner**: oneshot and worker-pool execution, kill+respawn recovery,
  env allowlisting, full failure taxonomy (ok/case_error/timeout/crash/protocol_error).
- **eval-graders**: 10 grader kinds incl. `judge_lite` stand-in; hand-rolled
  pass@k/Wilson/bootstrap statistics (exact reference values); baseline comparison
  with written evidence and uncertainty.
- **eval-cli**: `run|compare|report|init|projects|dashboard` commands, exit codes
  0/1/2, human + `--json` output.

### Multi-project portfolio

- Workspace registry (`eval-workspace.toml`) with name-spaced project IDs,
  relative/absolute repo paths, cross-project aggregation.
- `eval projects` lists status, counts, regression state.
- `eval dashboard` generates a self-contained static HTML page from
  canonical run stores (no server, no JS framework): project/suite/status
  filters, latency p50/p95, token/cost columns (`n/a` when absent),
  artifact links, failed-case details.

### Examples and integrations

- **LLM-call examples** (`examples/llm_calls/`): extraction, constrained
  classification, summarization via provider-neutral interface.
  Deterministic fake default; explicit opt-in live smoke (OpenAI-compatible,
  shape-only graders, bounded budget).
- **Real project integrations** (zero engine/project changes):
  - `cv_toolkit` CLI (validate/rescore)
  - `digest_agent` orchestrator offline (real LangGraph with own fakes)

### Protocol, conformance, and stability

- Committed `protocol/eval-protocol-v1.schema.json` with CI drift check.
- Python + Rust reference shims, parity-tested for byte-equivalent runs.
- Worker recovery: hang→Timeout, crash→Crash, malformed→ProtocolError,
  with no cross-contamination (proved on a shared single-worker process).

### Versioning policy

- **CLI**: semantic versioning. Additive changes only between minor versions;
  breaking changes require a major version bump.
- **Stored result format**: `schema_version` field in every run. Forward-only
  migrations on read; old runs are never rewritten.
- **Subprocess protocol**: `eval/v1` is stable. Additive changes are backward-tolerant
  (runner tolerates unknown fields); breaking changes become `eval/v2`.
  Major version negotiation is a runtime check, not a compile-time claim.

### Minimum supported Rust version

1.85, verified by CI.

### Defects fixed during RC2

- Runner env dropped home vars (NLTK-based CLI crashed under runner) — fixed,
  property tested.
- Portfolio totals counted every ref instead of newest-run-per-suite — fixed,
  cross-checked by independent Python audit.
- Deliberate-exit passes (nonzero exit graded by `exit_code`) counted as infra
  failures — fixed, tested.
- `stability` grader passed on all-crash trials — fixed, infra observations can
  never satisfy stability.
- Integration suites had machine-absolute paths — replaced with `$VAR` env
  references, tested for portability.
