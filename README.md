# eval-service — Reusable Rust Evaluation Platform

**Status: MVP implemented (M1–M8).** The review gate in `docs/08-decisions.md`
was approved with all recommended defaults; the engine below is the
implementation of `docs/06-roadmap.md` M1–M8.

One Rust engine evaluates heterogeneous projects through a small common
interface, detects meaningful regressions, explains its decisions, and
stays easy to extend. No rewrites, no shared framework, no server in v1.

Architectural principle: **a project implements a small documented
interface to become evaluable; the platform owns execution,
measurement, comparison, and reporting.**

## Quickstart

Prerequisites: Rust 1.85+ (see *Toolchain* below) and Python 3.11+
for the Python reference target.

```sh
cargo build -p eval-cli
./target/debug/eval run datasets/cv_demo/suite.yaml --ref baseline --set-baseline
# ... change something ...
./target/debug/eval run datasets/cv_demo/suite.yaml --ref work
./target/debug/eval compare --suite datasets/cv_demo/suite.yaml
# exit 0 = pass, 1 = regression/failure, 2 = infra error
```

New project in 5 minutes: `eval init mytool --dir .` scaffolds an
adapter script plus a suite; see `docs/tutorial.md`.

## Layout

```text
crates/
  eval-core/        domain types, suite config, workspace registry, file store, redaction
  eval-protocol/    versioned wire types, validation, conformance helpers
  eval-runner/      subprocess mgmt, worker pool, deadlines, failure taxonomy
  eval-graders/     graders, statistics, gate decision, portfolio aggregation, dashboard
  eval-cli/         eval run|compare|report|init|projects|dashboard, reports, exit codes
adapters/
  python_shim/      Python reference target (stdlib only)
  rust_shim/        Rust reference target (conformance-proven)
protocol/           eval-protocol-v1.schema.json (generated) + transcripts/
datasets/           cv_demo/, digest_demo/, security/ suites
examples/llm_calls/ LLM-app evaluation examples (offline fake + opt-in live)
eval-workspace.toml example portfolio registry (two demo projects)
```

## Commands

| Command | Purpose |
|---|---|
| `eval run <suite.yaml> [--ref work] [--store eval-runs] [--target ...] [--workers N] [--set-baseline] [--json]` | Execute, grade, store |
| `eval compare --suite <suite.yaml> [--baseline-ref baseline] [--candidate-ref work] [--json]` | Baseline diff + gate decision |
| `eval report --store <dir> --suite <name>... [--ref work] [--json]` | Render stored runs |
| `eval init <name> [--dir .]` | Scaffold adapter + suite + readme |
| `eval projects [--workspace eval-workspace.toml] [--json]` | Portfolio status across registered projects |
| `eval dashboard [--workspace eval-workspace.toml] [--out dashboard]` | Static HTML dashboard from canonical stores |

## Multi-project portfolio

Register projects (separate local repos welcome) in `eval-workspace.toml`:

```toml
[workspace]
name = "my portfolio"

[[project]]
id = "cv"
name = "CV toolkit"
repo = "../other/cv_toolkit"
suites = ["eval/suite.yaml"]
store = "eval-runs"
```

`eval projects` prints per-project status; `eval dashboard` writes a
single self-contained `index.html` (no server, no database, no JS
framework): portfolio totals with an honest pass-rate definition,
per-project runs with p50/p95 latency and token/cost columns (`n/a`
when never reported), failed cases with structured reasons, baseline
regression state, filterable history, and links to the stored
artifacts. Unavailable projects, missing suites, and corrupt records
are shown — never silently dropped. Totals count the newest run per
suite, so baseline/work histories never double-count.

Try it with the shipped registry (two demo projects):

```sh
eval run datasets/cv_demo/suite.yaml --ref baseline --set-baseline
eval run datasets/digest_demo/suite.yaml --ref baseline --set-baseline
eval dashboard --workspace eval-workspace.toml --out dashboard
# open dashboard/index.html
```

## LLM examples

`examples/llm_calls/` shows how an ordinary LLM app plugs in:
extraction, constrained classification, and coverage-graded
summarization behind a provider-neutral interface. Offline runs use a
deterministic fake (default, CI-covered); live calls need explicit
`EVAL_LLM_LIVE=1` plus `EVAL_LLM_API_KEY` and are a 3-request smoke
test with shape-only graders — see `examples/llm_calls/README.md`
for budgets, warnings, and what is (not) measured.

## Test battery (A1–A7, `docs/05-mvp.md`)

`cargo test --workspace` runs 90+ tests: grader units incl. adversarial
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

## Toolchain

- MSRV 1.85 (edition 2024), checked in CI.
- Windows: the GNU toolchain (`stable-x86_64-pc-windows-gnu`) plus a
  MinGW-w64 `gcc`/`dlltool` on PATH links out of the box; MSVC works
  with Visual Studio Build Tools. CI covers Windows + Linux.
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
  like the integration candidates' contracts; wiring the live
  `digest_agent`/`cv_toolkit` targets is `--target` configuration,
  not engine work.
- A deliberate nonzero exit that passes its `exit_code` grader (the
  conventional-CLI rejection pattern) is a graded pass, not an
  infrastructure failure. Portfolio error counts track failing cases
  with crash/protocol observations only.

## Documents

| # | File |
|---|---|
| A | `docs/01-inventory.md` |
| B | `docs/02-architecture.md` |
| C | `docs/03-protocol-v1.md` |
| D | `docs/04-domain-model.md` |
| E | `docs/05-mvp.md` |
| F | `docs/06-roadmap.md` |
| G | `docs/07-risks.md` |
| H | `docs/08-decisions.md` (approved: all recommended defaults) |
| I | `docs/09-recommendation.md` |
| T | `docs/tutorial.md` (zero-to-gated-suite walkthrough) |
| S | `docs/sample-report.md` (real command outputs) |
| C | `CONTRIBUTING.md` (workflow, scope discipline) |
