# AGENTS.md

Guidance for coding agents working in this repository. Humans: see `README.md`.

## What this is

`eval-service` is a Rust platform that evaluates other projects — and their
LLM usage — through one subprocess protocol (`eval/v1`). Adding a project
means writing an adapter + suite **here**; the Rust engine never changes.
Never modify a target project's repository to make it evaluable.

## Commands

```sh
make build                                   # the `eval` binary (target/debug/eval)
make quality                                 # fmt + clippy + check + tests + rustdoc
make verify                                  # alias for make quality
make check SUITE=path/to/suite.yaml          # validator + adapter probe
make help                                    # all developer/runner targets
```

On Linux/macOS/WSL, prefer `make` (it wraps the commands CI runs); raw
equivalents are `cargo fmt --all -- --check`, `cargo clippy --workspace
--all-targets --all-features -- -D warnings`, `cargo check --workspace
--all-targets --all-features`, `cargo test --workspace --locked
--all-targets --all-features`, and `RUSTDOCFLAGS="-D warnings" cargo doc
--workspace --no-deps --all-features`. On Windows without make, run those
directly. The Phase 3 runner gate is `make phase3-verify`.

MSRV is **1.85** (declared in `Cargo.toml`, verified by CI). Do not use
`let`-chains or other features newer than 1.85.

Live LLM observation: `eval proxy` (loopback reverse proxy — key
injection, model swap, SSE passthrough, budget reservation, one
llm/v1 event per attempt) and `eval cassette` (record with
`--cassette-dir`, replay offline with `origin=replay` — never live
evidence). See `docs/proxy.md`; the worked example is
`integrations/digest_agent/live_bench.yaml` through the proxy.

Python: stdlib only, `ruff` clean, under 100 columns, no shebang (files are
imported, not executable). `adapters/python_shim/`, `examples/`, and
`integrations/` are linted in CI.

## Adding an evaluation for a project

This is the most common task. The surface is small and self-describing:

```sh
eval schema                     # machine-readable: grader kinds + presets, field
                                # paths, protocol messages, suite keys, exit codes.
                                # Read this first.
eval onboard NAME --template cli|llm|agent --dir DIR
                                # one command: scaffold -> check --probe -> baseline
eval init NAME --template cli|llm|agent --dir DIR   # scaffold only
eval check SUITE.yaml --probe   # deterministic validator + adapter handshake test
eval run SUITE.yaml --ref baseline --set-baseline
eval compare --suite SUITE.yaml # exit 0 pass / 1 regression / 2 infra
```

Prefer presets over hand-written graders: `presets: [cli_contract, stable,
no_secret_leak, json_object:{required:[..]}]` (full list in `eval schema`).

Rules that the tooling enforces or expects:

- The adapter writes **eval/v1 JSONL to stdout only**; logs go to stderr. The
  engine kills on deadline, classifies crashes/timeouts/protocol errors, and
  never merges them with handled `case_error`.
- Grade **properties, never exact wording**, for model output. Use
  `json_schema`, `regex`, `range`, `judge_lite`; use `trials >= 2` +
  `stability` for nondeterministic targets.
- Never fabricate metrics. Token counts come from the provider's own usage
  block; absent metrics stay `n/a` (see `docs/llm-events.md`).
- Adapters live under `integrations/<project>/` in **this** repo. Suites too.
- `eval check --probe` is the gate — converge until it exits 0 before grading.

Reference integrations to copy from: `integrations/research_context/`
(deterministic CLI) and `integrations/digest_agent/` (agent workflow + usage
decorator).
Pattern contracts: `docs/integration.md`.

## Do not

- Do not edit generated files: `protocol/eval-protocol-v1.schema.json` is
  generated. Regenerate with
  `cargo run -p eval-protocol --example generate_schema`; a test enforces it.
- Do not commit secrets. `.env` is git-ignored; live credentials exist only in
  the launching environment. `gitleaks` runs in pre-commit and CI. See
  `SECURITY.md`. Never print or commit a key.
- Do not modify `digest_agent` or any target repository — inspect and run it,
  never edit it.
- Do not add dependencies without need; the engine is deliberately lean
  (`serde`, `tokio`, `clap`, `regex`, `blake3`, `toml`).

## Validate before finishing

Run `make quality` (fmt + clippy + check + full tests + rustdoc), or
`make verify` as an alias. If you added or changed an integration, also
run `make check SUITE=<suite>` and a baseline→work→compare cycle (or
`eval check --probe` directly). For gate/PR changes, run the relevant
`eval gate`/`eval notify` tests and see `docs/pr-gate.md`. Keep the
working tree clean of `eval-runs/`, `target/`, and `.env`.
