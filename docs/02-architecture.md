# Architecture

## Goal

One Rust engine evaluates heterogeneous projects through a small,
versioned subprocess contract (`eval/v1`), measures LLM usage, detects
regressions with evidence, and stays easy to extend. Projects keep their
own languages, frameworks and repositories.

## Workspace

```
eval-service/
├── crates/
│   ├── eval-core/       domain types, suite config, workspace registry, file store, redaction
│   ├── eval-protocol/   eval/v1 wire types, validation, generated JSON Schema
│   ├── eval-runner/     subprocess execution, worker pool, deadlines, Docker sandbox
│   ├── eval-graders/    graders, statistics, gate decisions, calibration, portfolio/dashboard
│   ├── eval-usage/      llm/v1 events, idempotent store, pricing, aggregation, collector
│   ├── eval-proxy/      loopback OpenAI-compatible observer, budgets, cassettes
│   └── eval-cli/        eval command surface, reports, exit codes
├── adapters/
│   ├── python_shim/     reference target (stdlib-only Python)
│   └── rust_shim/       reference target (Rust, conformance-tested)
├── integrations/        real project adapters and suites (thin, project-specific)
├── infra/               AWS runner Terraform, systemd units, Docker sandbox image
├── protocol/            generated eval-protocol-v1 schema + transcripts
├── datasets/            demo, digest, security and calibration fixtures
├── docs/                contracts, runbooks, phase history, quality gates
└── .github/workflows/   ci-fast, terraform, terraform-plan, terraform-drift, pr-gate
```

There are seven library/CLI crates plus one adapter crate. Split a crate
only when a boundary strains; the file store stays in `eval-core` until
SQLite or concurrent writers are actually required.

## Component contracts

### `eval-core` — domain and persistence
- **Responsibility:** suite/case/limits/grader schema, `Suite`,
  `RunRecord`, `CaseRecord`, `TrialObservation`, workspace registry,
  file-backed `RunStore`, baseline pointers, redaction, stable IDs.
- **Boundary:** sync and dependency-light; no process spawning, grading,
  network, report rendering or CLI concerns.
- **Store layout:**
  `runs/<suite>/<ref>/run-<id>/metadata.json`, `observations.jsonl`,
  `grades.json`, optional `decision.json` and `provenance.json`;
  `baselines/<suite>.json` is a pointer, never a copy.
- `RunStore::save` redacts before hashing and assigns a content-derived
  run id; old records are read forward, never rewritten.

### `eval-protocol` — the wire contract
- **Responsibility:** `eval/v1` handshake/case/event/result/case_error/
  shutdown messages, version negotiation, validation, conformance
  helpers, generated JSON Schema.
- **Boundary:** used by `eval-runner` and both reference adapters.
  Generated schema is drift-checked by a test; do not edit it by hand.

### `eval-runner` — execution
- **Responsibility:** spawn targets, worker pool, deadlines with
  kill/respawn, stdout/stderr separation, env allowlisting, failure
  taxonomy (`ok`, `case_error`, `timeout`, `crash`, `protocol_error`),
  and the Docker sandbox.
- **Sandbox:** `RunPolicy.sandbox` adds a fresh container with an
  internal network, read-only mounts, non-root user, dropped
  capabilities, PID/memory/CPU limits, no Docker socket and no host
  network. Sandbox supports oneshot targets only.
- **Boundary:** it owns *how* a target runs, never what "correct" means.

### `eval-graders` — judgment
- **Responsibility:** deterministic graders (exit code, exact, regex,
  JSON Schema, range, stability), judge-lite, hand-rolled statistics
  (Wilson, paired bootstrap, pass@k), gate decisions with evidence,
  calibration diagnostics, portfolio aggregation and dashboard data.
- **Gate:** `decide_gate(baseline, candidate, policy) -> GateDecision`
  explains pass/fail, affected cases, evidence and uncertainty. A
  missing baseline means no regression claim.
- **Calibration:** κ/CI and A/B bias are advisory unless a real
  human-labelled dataset clears the configured bar. No labels means no
  promotion.

### `eval-usage` — LLM usage
- **Responsibility:** versioned `llm/v1` events, idempotent event store,
  pricing, per-project/model/task/context aggregation, loopback collector.
- **Honesty rule:** tokens/costs come from provider usage or documented
  pricing; absent values stay `n/a`; production, benchmark and dev
  contexts never mix silently.

### `eval-proxy` — live observation
- **Responsibility:** loopback OpenAI-compatible reverse proxy, provider
  key injection, model swap, SSE passthrough, reservation/reconciliation
  budgets, one `llm/v1` event per attempt, cassette record/replay.
- **Boundary:** keys stay in the launching environment. Replay runs are
  stamped `origin=replay` and never count as live evidence.

### `eval-cli` — user surface
- **Responsibility:** `run`, `compare`, `gate`, `check`, `report`, `init`,
  `onboard`, `projects`, `dashboard`, usage commands, `proxy`, `cassette`,
  `collect`, `watch`, `notify`, `calibrate`, `schema`.
- **Exit codes:** `0` pass, `1` regression/failure/policy refusal,
  `2` infrastructure/configuration/protocol error.
- **Gate provenance:** `eval run --origin live|replay` writes
  `provenance.json`. `eval gate --promote` accepts only live,
  `refs/heads/main`, push/manual, sandboxed runs. Replay is never
  promotable.
- **Watch/notify split:** `watch` is read-only and uses
  `EVAL_WATCH_TOKEN`; `notify` uses a separate narrow-write
  `EVAL_GITHUB_WRITE_TOKEN` and idempotent comment/status markers.

## Runtime flows

### Suite execution
```
suite YAML ──► eval-cli ──► eval-runner ──eval/v1──► adapter ──► project
                  │              │
                  │              ├─ native process, or Docker sandbox
                  │              └─ proxy sidecar for live model calls
                  ▼
             raw observations ──► eval-graders ──► cases + gate
                  │
                  └──► eval-core store (metadata/observations/grades/decision)
```

### Regression gating
```
main run (live, sandbox) ──► provenance + baseline pointer
                                   │
PR run (replay, sandbox) ──► eval gate ──► pass/regression ──► CI status
```
The PR gate never receives live provider keys. Live baseline promotion
is a separate, explicit operation; a PR cannot promote.

### Watch queue
```
GitHub issues (read-only) ──► eval watch ──► allowlisted suite
                                  │             │
                                  │             └─ Docker sandbox
                                  └─ state.json + heartbeat + S3 store sync
```
Queue state is restart-safe and de-duplicates by request id. Comments
and statuses live only in `eval notify`, using a different credential.

## Trust and configuration boundaries

- **Untrusted target code:** always assume target/adapter code is
  untrusted. The Docker sandbox is the execution boundary; a subprocess
  without it is not a security sandbox.
- **PR jobs:** `pull_request` only, `contents: read`, no secrets, no
  OIDC, no live provider keys; the agent uses the offline classify suite.
- **AWS CI:** OIDC only. `eval-github-plan` can read state and write the
  `.tflock`; it cannot write the state object. No apply role exists yet.
- **Watcher:** read-only GitHub token; must fail closed without it.
- **Secrets:** never print/commit keys; redaction is tested; gitleaks
  scans full history in CI.

## Quality gates

`make quality` is the Rust gate: fmt, clippy (all targets/features,
`-D warnings`), type-check, all tests, and rustdoc with warnings denied.
`make verify` is an alias. CI adds MSRV 1.85, Windows, ARM64, offline,
Python lint, gitleaks, actionlint, Terraform, and the replay PR gate.
See `docs/quality.md`.

## Trade-offs

1. **Subprocess + JSONL over in-process plugins.** One spawn/JSON cost
   buys language agnosticism and blast-radius separation.
2. **File store over SQLite.** Diffable, zero setup, sufficient for a
   single writer; revisit on concurrent writers or ad-hoc queries.
3. **Sync core, async runner/proxy.** Domain and grading stay easy to
   test; only execution/network paths are async.
4. **Hand-rolled statistics.** Wilson/bootstrap/pass@k are small, tested
   functions; no heavy stats dependency until justified.
5. **No target-repo coupling.** Integrations are thin adapters in this
   repository; the engine never edits a target project.
6. **No apply automation yet.** Plan/drift are read-only; apply remains
   a deliberate future lane with a separate role and protected
   environment.
