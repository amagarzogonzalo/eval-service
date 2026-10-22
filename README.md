# eval-service — Cross-Project LLM Usage, Benchmarking & Performance Platform

**Status: private production service.** A Rust evaluation
engine runs in a private, token-authenticated service on an AWS ARM64 VM,
with weekly GitHub Actions runs, S3 evidence mirroring, Terraform-managed
infrastructure, and a read-only evidence UI. Reusable and
language-agnostic: projects are evaluated through a common interface, and
the platform owns execution, measurement, comparison, and reporting.

> Build a reusable, language-agnostic platform that collects LLM usage
> from different projects through a common interface, measures the
> quality and performance of LLM-powered applications, and enables
> consistent comparisons of models, prompts, costs, latency and
> reliability across projects.

One Rust engine evaluates heterogeneous projects through a small common
interface, detects meaningful regressions, explains its decisions, and
stays easy to extend. The projects it measures can use Python, Rust,
LangGraph, custom provider clients, or other frameworks. It is not an
agent framework or just a test runner — it is the common measurement
and evaluation layer shared by your projects.

Two data paths, one dashboard:

- **Usage tracking** — instrumented projects report per-call `llm/v1`
  events (tokens, latency, errors, cost); the platform aggregates by
  project, model, provider, task and day. Answers *"what does each
  project consume and what does it cost?"*
- **Benchmarking** — controlled suites grade output quality while the
  same runs record usage; quality-vs-cost-vs-latency comparisons join
  the two. Answers *"which variant performs best, and did a change
  regress anything?"*

Architectural principle: **a project implements a small documented
interface to become evaluable; the platform owns execution,
measurement, comparison, and reporting.**

## Production (private service)

The operator path starts at [`docs/production.md`](docs/production.md).
The production UI is private: SSH tunnel on local port `19090`, Caddy
Basic Auth, then an application API token/session. No token is stored in
this repository.

| Task | Command / entry point | Read |
|---|---|---|
| Trigger the weekly run now | `gh workflow run live-production.yml` | [`production.md`](docs/production.md) |
| Run one lane manually | `sudo systemctl start eval-live-digest.service` | [`production.md`](docs/production.md) |
| Open evidence | tunnel `19090 -> 127.0.0.1:8080` | [`private-access.md`](docs/private-access.md) |
| Interpret runs/charts | current vs legacy rules | [`benchmark-evidence.md`](docs/benchmark-evidence.md) |
| Verify a RAG answer | source-by-source checklist | [`rag-verification.md`](docs/rag-verification.md) |
| Recover from a failure | triage and rollback | [`troubleshooting.md`](docs/troubleshooting.md) |

Current production evidence rules: free-only models with no paid
fallback; deterministic checks gate; the content judge is advisory;
missing metrics are `n/a`, never zero; current and legacy runs are
separated.

## Quickstart

Prerequisites: Rust 1.85+ (see *Toolchain* below) and Python 3.11+
for the Python reference target.

```sh
make build    # or: cargo build -p eval-cli
./target/debug/eval run datasets/cv_demo/suite.yaml --ref baseline --set-baseline
# ... change something ...
./target/debug/eval run datasets/cv_demo/suite.yaml --ref work
./target/debug/eval compare --suite datasets/cv_demo/suite.yaml
# exit 0 = pass, 1 = regression/failure, 2 = infra error
```

On Linux/macOS/WSL, `make check SUITE=...`, `make baseline`, `make work`,
and `make compare` wrap the same commands; `make quality` is the full
fmt + clippy + check + tests + rustdoc gate and `make help` lists every
target. Windows without make uses the raw cargo/eval commands.

New project in 5 minutes: `eval init mytool --template cli` scaffolds a
runnable integration; `eval schema` describes the whole surface and
`docs/integration.md` explains it. The documentation index is
`docs/README.md`.

## Layout

```text
crates/
  eval-core/        domain types, suite config, workspace registry, file store, redaction
  eval-protocol/    versioned wire types, validation, conformance helpers
  eval-runner/      subprocess mgmt, worker pool, deadlines, failure taxonomy
  eval-graders/     graders, statistics, gate decision, portfolio aggregation, dashboard
  eval-usage/       llm/v1 events, idempotent store, pricing, aggregation, collector
  eval-proxy/       loopback OpenAI-compatible observer, budgets, cassettes
  eval-cli/         eval run|compare|report|init|projects|dashboard|usage-*, reports, exit codes
adapters/
  python_shim/      Python reference target (stdlib only)
  rust_shim/        Rust reference target (conformance-proven)
integrations/       real project adapters + suites (thin, target-specific)
infra/aws/          Terraform + cloud-init for the ARM64 runner (+ bootstrap)
infra/observability/ Phoenix + OTel Collector + Caddy stack
web/                public benchmark page + private evidence UI/API
infra/docker/       sandbox target image for `eval run --sandbox docker`
docs/               protocol, integration, proxy, observability, quality, runbooks, design history
protocol/           eval-protocol-v1.schema.json (generated) + transcripts/
datasets/           cv_demo/, digest_demo/, security/ suites
examples/llm_calls/ LLM-app evaluation examples (offline fake + opt-in live + fixture benchmark)
pricing/prices.toml versioned pricing table (illustrative fixture rates)
eval-workspace.toml example portfolio registry (two demo projects)
Makefile            canonical developer/runner targets
```

## Commands

| Command | Purpose |
|---|---|
| `eval schema [--compact]` | Machine-readable platform manifest (graders, presets, fields, protocol, exit codes) — read this first |
| `eval onboard <name> --template cli\|llm\|agent [--dir .]` | Zero to verified baseline: scaffold → check --probe → baseline |
| `eval init <name> --template cli\|llm\|agent [--dir .] [--force]` | Scaffold a runnable adapter + suite + readme |
| `eval check <suite.yaml> [--probe] [--json]` | Deterministic suite validator + adapter handshake test |
| `eval run <suite.yaml> [--ref work] [--store eval-runs] [--target ...] [--workers N] [--set-baseline] [--json]` | Execute, grade, store; add `--sandbox docker` for the isolated executor |
| `eval run ... --sandbox docker --sandbox-network <internal> --sandbox-mount <path>` | Run a oneshot target with proxy-only network, read-only rootfs, no DNS/metadata/Docker socket |
| `eval compare --suite <suite.yaml> [--baseline-ref baseline] [--candidate-ref work] [--json]` | Baseline diff + gate decision |
| `eval gate --suite <suite.yaml> [--origin live\|replay] [--promote]` | Enforce provenance policy; promote only a live, main, sandboxed run |
| `eval notify --repo <owner/name> [--issue N --body TEXT] [--status-sha SHA ...]` | Idempotent comment/status with a separate narrow-write token |
| `eval report --store <dir> --suite <name>... [--ref work] [--json]` | Render stored runs |
| `eval projects [--workspace eval-workspace.toml] [--json]` | Portfolio status across registered projects |
| `eval dashboard [--workspace eval-workspace.toml] [--out dashboard]` | Static HTML dashboard from canonical stores |
| `eval usage-import <file> --project <id> [--tag T] [--run-id R] [--suite S] [--ref R]` | Validate + idempotently store usage events |
| `eval usage-report --project <id> [--model M] [--tag T] [--context C] [--pricing F] [--json]` | Usage aggregates (tokens, cost, latency, tasks) |
| `eval proxy [OPTIONS] --key-env <VAR> --project <id> <upstream>` | Loopback OpenAI-compatible observer; one llm/v1 event per attempt |
| `eval cassette <dir> --project <id> [--port 8788]` | Offline replay of recorded exchanges; hits stamp `origin=replay` |
| `eval collect [--port 4317] [--token-file F]` | Loopback-only local event collector |
| `eval watch --config eval-watch.toml [--once] [--dry-run]` | Read-only GitHub issue polling, debounced restart-safe queue, heartbeat |
| `eval calibrate <labels.jsonl> --out <report.json> [--markdown <report.md>]` | Judge calibration: Cohen's kappa + CI, A/B swap, thresholds, advisory decision |

## Integrating a project (agent-friendly)

The whole integration surface is self-describing and validated by tooling —
an agent (or a human) needs no prose and no engine edits:

```sh
eval onboard mytool --template cli   # scaffold + check --probe + baseline, one command
# later, to gate changes:
eval run mytool/suite.yaml --ref work
eval compare --suite mytool/suite.yaml   # 0 pass / 1 regression / 2 infra
```

`eval schema` describes the surface; grader **presets** cut suite boilerplate:

```yaml
cases:
  - id: x
    presets: [cli_contract, nonempty_output, no_secret_leak]
    graders:
      - kind: exact
        field: output.text
        value: "expected"
```

`eval check` returns stable issue codes with fix hints (bad regex, unknown
preset, stability without trials, unknown field roots, adapter handshake
failures), so iteration is a deterministic loop, not guesswork. See
`docs/integration.md` for the contract and `AGENTS.md` for the agent workflow.

## Multi-project portfolio

Register projects (separate local repos welcome) in `eval-workspace.toml`:

```toml
[workspace]
name = "my portfolio"

[[project]]
id = "other"
name = "Other project"
repo = "../other-project"
suites = ["eval/suite.yaml"]
store = "eval-runs"
```

`eval projects` prints per-project status; `eval dashboard` writes a
single self-contained `index.html` (no server, no database, no JS
framework): portfolio totals with an honest pass-rate definition,
per-project runs with p50/p95 latency and token/cost columns (`n/a`
when never reported), lane/model/free-only labels, quality vs latency,
service availability and cold-start/queue-wait columns, failure-class
counts, failed cases with structured reasons, baseline regression state
(only for like-for-like runs), filterable history, and links to the
stored artifacts, trace, and S3 evidence, plus a last-30-runs trend
per suite. Unavailable projects, missing suites, corrupt records, and
corrupt `manifest.json` sidecars are shown — never silently dropped.
Totals count the newest run per suite, so baseline/work histories never
double-count. CLI runs write `manifest.json`; `eval manifest-backfill`
fills legacy stores. See `docs/run-metrics.md` for the versioned
`run-manifest/v1` contract and JSON Schema.

Try it with the shipped registry (two demo projects):

```sh
eval run datasets/cv_demo/suite.yaml --ref baseline --set-baseline
eval run datasets/digest_demo/suite.yaml --ref baseline --set-baseline
eval dashboard --workspace eval-workspace.toml --out dashboard
# open dashboard/index.html
```

## LLM usage and benchmarking

`eval-usage` collects per-call `llm/v1` events (tokens, latency,
errors, cost — see `docs/llm-events.md`) via JSONL import
(`eval usage-import`) or the loopback collector (`eval collect`).
Aggregates never fabricate: absent metrics render `n/a`, reported
and estimated costs stay distinct with pricing versions attached,
and production/benchmark/dev traffic aggregates separately.

`examples/llm_calls/` shows how an ordinary LLM app plugs in:
extraction, constrained classification, coverage-graded summarization,
and a two-variant fixture benchmark (quality vs. cost vs. latency)
behind a provider-neutral interface. Offline runs use a deterministic
fake (default, CI-covered); live calls need explicit `EVAL_LLM_LIVE=1`
plus `EVAL_LLM_API_KEY` and are a 3-request smoke test with
shape-only graders — see `examples/llm_calls/README.md` for budgets,
the operator procedure, and what is (not) measured. The examples'
live smoke has not run here: the mock-transport tests prove the
pipeline, not any provider. Real integration live runs are documented
in `docs/proxy.md` and `infra/aws/README.md`.

## Test battery

`make test` (`cargo test --workspace --locked --all-targets --all-features`) runs the full workspace suite: usage contract/import/
pricing/aggregation/collector units, grader units incl. adversarial
inputs, stats reference values (pass@k, Wilson, seeded bootstrap),
synthetic stable/noisy/degraded gate datasets, conformance
(echo/hang/crash/malformed/wrong-version/unknown-field), worker
recovery with no cross-contamination, Rust↔Python shim parity,
thin-adapter (A1), intentional regression (A2), quiet reruns with
identical digests (A3), two-project report (A7), workspace registry
and portfolio aggregation (missing/corrupt/duplicate handling),
dashboard rendering cross-checked against stores, and offline LLM
examples with a live-mode opt-in guard. Offline by
construction: no test makes network calls (A6; CI also runs a
no-network job).

## Quality gates

`make quality` (alias `make verify`) runs formatting, clippy with all
targets/features and warnings denied, `cargo check`, the full test suite,
and rustdoc with warnings denied. CI repeats this on Linux, Windows and
ARM64, plus MSRV 1.85, offline, Python, gitleaks, actionlint, Terraform,
and the replay-only PR gate. See `docs/quality.md` for the exact matrix
and known gaps.

## Production engineering (implemented)

This repository is also a production operations exercise; the following
are implemented here, not slideware:

- **Rust engine**: a workspace of focused crates (`eval-core`,
  `eval-protocol`, `eval-runner`, `eval-graders`, `eval-usage`,
  `eval-proxy`, `eval-cli`) with a versioned subprocess protocol,
  conformance/parity tests, immutable run storage, redaction, and
  deterministic replay gates.
- **Terraform + AWS**: VPC, an EC2 `t4g.small` ARM64 runner, an
  encrypted/versioned private S3 evidence bucket, IAM instance profile,
  SNS alarms, a budget with a stop action, and OIDC-based read-only
  Terraform plan/drift workflows. Apply remains manual.
- **Scheduling and operations**: weekly GitHub Actions on a self-hosted
  ARM64 runner, systemd oneshot lanes, a private read-only UI behind
  Caddy Basic Auth plus an API token/session, Phoenix/OTel traces, and
  an append-only S3 mirror.
- **Measurement discipline**: free-only model policy (no paid
  fallback), `n/a` for unobserved metrics, current/legacy separation,
  deterministic gates, and advisory-only content judges.

See [`docs/production.md`](docs/production.md) for the topology and
operator path. The service is a single ARM64 node, not a highly
available cluster.

## Toolchain

- MSRV 1.85 (edition 2024), checked in CI.
- Windows: the GNU toolchain (`stable-x86_64-pc-windows-gnu`) plus a
  MinGW-w64 `gcc`/`dlltool` on PATH links out of the box; MSVC works
  with Visual Studio Build Tools. CI covers Windows, Linux and ARM64.
- `$PYTHON` in suite target argv resolves via `EVAL_PYTHON`, else
  `python3`/`python` on PATH.

## Honest boundaries

- A subprocess is **not a sandbox**: env allowlisting, cwd confinement,
  and redaction are implemented; OS-level isolation is required before
  running untrusted targets.
- Statistics are hand-rolled Wilson/bootstrap/pass@k with reference
  tests; per-case output is diagnostic-only and every gate failure
  cites evidence lines.
- `judge_lite` is a deterministic stand-in (citations + length); live
  judges stay an explicit opt-in outside v1.
- The demo suites exercise the engine through reference shims shaped
  like the integration candidates' contracts; the live `digest_agent`
  target runs through a thin adapter in `integrations/` with zero
  changes to either project or the engine.
- A deliberate nonzero exit that passes its `exit_code` grader (the
  conventional-CLI rejection pattern) is a graded pass, not an
  infrastructure failure. Portfolio error counts track failing cases
  with crash/protocol observations only.
- The production service is a single `t4g.small` VM with no HA/failover;
  the AWS free trial ends 2026-12-31. Terraform apply is manual, and no
  automated retention job runs.
- Remote research records the upstream model as `target-service:remote`
  because the deployed target does not expose it; internal tokens/cost
  are `n/a`. Research retrieval can be flaky and the production service
  suite still hardcodes the CRISPR topic, so RAG production evidence is
  not yet a completed migration.
- Fixture benchmarks demonstrate the quality/cost/latency machinery;
  they are labeled synthetic everywhere and are never evidence about
  real models. Statistical gate behavior is covered by synthetic
  datasets; false-positive/negative rates on real workloads are
  unmeasured. No human-labelled calibration dataset exists, so the judge
  remains advisory and is never promoted (see `docs/quality.md`).

## Documents

Start at `docs/README.md`. The production path is:

- `docs/production.md` - topology, weekly schedule, deploy/verify/rollback
- `docs/private-access.md` - tunnel, ports, Basic Auth and API token/session
- `docs/benchmark-evidence.md` - current/legacy, models, filters, judges
- `docs/rag-verification.md` - source-by-source RAG verification
- `docs/troubleshooting.md` - triage, recovery, escalation evidence
- `docs/legacy-runs.md` - legacy policy, UI behavior, backfill

Platform contracts:

- `docs/integration.md` — adapter/suite contract and workflow
- `docs/02-architecture.md` — current crate boundaries and runtime flows
- `docs/03-protocol-v1.md` / `docs/04-domain-model.md` — wire and result contracts
- `docs/proxy.md` / `docs/llm-events.md` — live observation and usage truth
- `docs/sandbox.md` / `docs/watch.md` / `docs/pr-gate.md` — isolated execution, watch, PR gate
- `docs/10-cicd-aws.md` — OIDC plan/drift lane and AWS controls
- `docs/quality.md` — Rust quality gates, CI matrix and known gaps
- `docs/judge-calibration.md` — advisory calibration (no human labels yet)
- `AGENTS.md` — coding-agent workflow
- `CHANGELOG.md` / `CONTRIBUTING.md` — release and contribution policy

`docs/01-inventory.md` through `docs/09-recommendation.md` are the
original design record; treat them as historical context.
