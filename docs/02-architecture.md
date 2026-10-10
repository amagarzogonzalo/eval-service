# B. Architecture decision document

## Goal restated

One Rust engine evaluates heterogeneous projects through a small
common interface, detects meaningful regressions, explains its
decisions, and stays easy to extend. No rewrites, no shared
framework, no server in v1.

## Recommended workspace (5 crates, consolidated)

The brief suggested 7 crates. Recommendation: **5** — `evals-store`
merges into `eval-core` (file persistence is ~200 lines; a separate
crate buys nothing until SQLite), and `evals-compare` merges into
`eval-graders` (comparison operates on graded results; same versioning
concerns). Split only when a boundary strains.

```
eval-service/                # this repo (planning docs live in docs/)
├── crates/
│   ├── eval-core/           # domain types, suite config, result model, file store, redaction
│   ├── eval-protocol/       # versioned wire types, validation, conformance helpers
│   ├── eval-runner/         # subprocess mgmt, worker pool, deadlines, failure taxonomy
│   ├── eval-graders/        # grader trait + deterministic graders, stats, judge, calibration
│   └── eval-cli/            # commands, reports (human + JSON), exit codes
├── adapters/
│   ├── python_shim/         # reference target (Ruff-clean)
│   └── rust_shim/           # reference target (in-process trait path)
├── protocol/                # eval-protocol-v1.schema.json (generated) + transcripts/
├── datasets/                # cases/, security/, calibration/, fixtures/
└── .github/workflows/       # ci-fast.yml, ci-evals.yml, release.yml
```

## Component contracts

### `eval-core` — domain, config, results, store
- **Responsibility:** case/suite schema (`Suite`, `Case`, `Limits`,
  `GraderRef`), `RunRecord`/`CaseResult` types, file store layout,
  secret redaction, schema versioning + forward-only migrations.
- **Public interface:** `load_suite(path) -> Suite`,
  `store.save(run) -> RunId`, `store.load(id)`, `store.baseline(suite)`,
  `redact(value) -> value`.
- **Dependencies:** `serde`, `serde_json`, `thiserror`, `blake3`
  (stable content hashes for run IDs and cassette keys).
- **Relations:** depended on by every other crate; depends on none.
- **Inside:** types, config parsing, store, redaction, migrations.
  **Outside:** process spawning, grading logic, presentation.
- **Tested by:** unit tests on parsing/validation/migration; property
  tests (redaction never leaks secret patterns; store round-trips).

### `eval-protocol` — the wire contract
- **Responsibility:** v1 message types (handshake, case, event, result,
  case_error, shutdown), validation, version negotiation, conformance
  target helpers (echo/hang/crash/malformed fixtures).
- **Public interface:** `negotiate(handshake) -> Mode`,
  `parse_line(line) -> Inbound`, `encode_outbound(msg) -> String`,
  `validate_case(case) -> Vec<SchemaError>`.
- **Dependencies:** `serde`, `serde_json`, `thiserror`, `schemars`
  (schema generation only).
- **Relations:** used by `eval-runner`; schema JSON committed to
  `protocol/` and drift-checked in CI.
- **Inside:** message types, validation, versioning.
  **Outside:** spawning, timeouts, grading.
- **Tested by:** conformance suite (both shims must produce
  schema-equal runs on the same fixture); unknown-field tolerance
  tests; version-mismatch rejection tests.

### `eval-runner` — execution
- **Responsibility:** spawn targets, worker pool (default
  `min(4, parallelism)`), per-case deadlines with kill+respawn,
  stdout/stderr separation, failure taxonomy
  (ok / case_error / timeout / crash / protocol_error — never merged).
- **Public interface:** `run_suite(suite, targets, policy) -> RunRecord`
  (async, tokio), `RunPolicy { workers, timeout_ms, env_allowlist }`.
- **Dependencies:** `tokio`, `eval-core`, `eval-protocol`.
- **Relations:** drives targets; hands raw observations to graders.
- **Inside:** processes, deadlines, pool, taxonomy.
  **Outside:** what "correct" means.
- **Tested by:** integration tests with hang/crash/malformed targets;
  timeout-vs-graceful-budget distinction test; no-network default.

### `eval-graders` — judgment
- **Responsibility:** `Grader` trait + deterministic graders
  (exit-code, exact, regex, JSON-schema, range, stability),
  statistics (pass@k unbiased, Wilson CI, paired bootstrap,
  Newcombe diagnostic), judge client trait + calibration
  (κ, position/verbosity bias), suite-gate decision with explanation.
- **Public interface:** `grade(case_result, graders) -> Grades`,
  `decide_gate(baseline, candidate, policy) -> GateDecision { pass,
  what_changed, affected_cases, evidence, uncertainty }`.
- **Dependencies:** `eval-core`; statistics hand-rolled (no `statrs`
  in v1 — see `docs/08-decisions.md`); HTTP only behind the judge
  trait, fake judge in tests.
- **Relations:** consumes runner output; feeds `eval-cli` reports.
- **Inside:** graders, stats, calibration, gate logic.
  **Outside:** spawning, storage, presentation.
- **Tested by:** grader unit tests incl. adversarial inputs;
  synthetic stable/noisy/degraded datasets proving the gate fires on
  real drift and stays quiet on noise.

### `eval-cli` — user surface
- **Responsibility:** `run`, `compare`, `report`, `init` commands;
  human + `--json` reports; documented exit codes
  (0 pass, 1 regression/failure, 2 infra/protocol error).
- **Dependencies:** all of the above; `clap`, `anyhow`.
- **Relations:** thin; no logic that belongs in the libraries.
- **Inside:** arg parsing, output formatting, exit codes.
  **Outside:** everything else.
- **Tested by:** `assert_cmd`-style CLI tests incl. the
  intentional-regression demo (break a fixture → exit 1 with
  explanation).

## Key trade-offs

1. **Subprocess + JSONL over in-process plugins.** Costs a spawn per
   worker and JSON parsing; buys language agnosticism (the primary
   objective) and blast-radius isolation. Worker mode amortizes spawn
   cost; oneshot stays for stateless targets.
2. **File store over SQLite in v1.** Zero setup, diffable, git-friendly;
   sufficient until >10k runs, concurrent writers, or ad-hoc queries
   (migration trigger, `docs/08-decisions.md`). Layout:
   `runs/<suite>/<ref>/run-<ts>-<id>.jsonl` + `metadata.json`,
   `baselines/<suite>.json` (pointer, not copy).
3. **Sync core, async runner.** Domain/store/graders are sync and
   trivially testable; only `eval-runner` (and the judge client) are
   async. Keeps the dependency surface minimal.
4. **Hand-rolled statistics in v1.** Wilson, bootstrap, and pass@k are
   each <100 lines with exact unit tests against reference values;
   avoids a heavy dependency for three functions. Revisit if Bayesian
   or sequential methods are later justified by measured gate performance.
5. **No harness coupling.** The optional Rust agent harness
   (§9 of the brief) is a future *target* of this platform, never a
   dependency. Nothing in these crates knows about agents.

## Diagram

```
suite YAML ──► eval-cli ──► eval-runner ──stdin/stdout──► target adapter ──► project
  (cases,          │              │  worker pool,                (python/rust/any)
   graders,        │              │  deadlines, taxonomy
   policy)         │              ▼
                   │         raw observations
                   │              │
                   │              ▼
                   │         eval-graders ──► grades + gate decision
                   │              │
                   └────── eval-core (types, store, redaction) ──► runs/ + report
```
