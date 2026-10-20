# Quality gates

The repository is small enough to keep the quality gate strict. A change
is not done until the local gate and the CI matrix are green.

## Local gate

```sh
make quality
```

This runs:

| Gate | What it proves |
|---|---|
| `cargo fmt --all -- --check` | deterministic formatting |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | no lint warnings in code, tests, examples or adapters |
| `cargo check --workspace --all-targets --all-features` | every target type-checks |
| `cargo test --workspace --locked --all-targets --all-features` | unit, integration, CLI, conformance and documentation-relevant tests |
| `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features` | no broken intra-doc links, bare URLs or rustdoc warnings |

`make verify` is an alias for `make quality`. On Windows without make,
run the five commands directly.

The workspace also denies `unsafe_code`, `dbg_macro`, `todo!` and
`unimplemented!`. Two integration-test crates explicitly allow
`unsafe_code` because Rust 2024 requires `unsafe` for
`std::env::set_var`; production code has no unsafe blocks.

## CI gates

`ci-fast` runs:

- Rust on Ubuntu and Windows: the same quality gate (Windows runs the raw
  commands, including rustdoc).
- ARM64 (`ubuntu-24.04-arm`): the full `make verify` gate on the
  delivery architecture.
- MSRV 1.85: the full test suite on the declared minimum toolchain.
- Offline: the workspace suite with a dead proxy and `CARGO_NET_OFFLINE`.
- Python: `ruff check` and `compileall` for adapters/examples/integrations.
- Secrets: gitleaks over full git history.

`terraform` validates both stacks without credentials.
`terraform-plan`/`terraform-drift` run read-only OIDC plans on `main`.
`pr-gate` runs the replay-only offline classify suite through the Docker
sandbox and gates the regression; it is a required check.

## Repository audit

The pre-gate audit covered every tracked file:

- 175 tracked files after this change.
- JSON, JSONL, YAML and TOML all parse.
- Every relative Markdown link resolves.
- No trailing whitespace in text/source files.
- No `TODO`/`FIXME`/`dbg!`/`todo!`/`unimplemented!` markers.
- `gitleaks` full history and working tree: clean.
- `actionlint`: clean.
- Terraform fmt/validate and Python ruff/compile: clean.

The rustdoc gate immediately found three real issues (two `argv[0]`
intra-doc links and one bare URL); those are fixed in the same change.

## Known gaps

These are deliberate, not hidden:

- No `cargo-deny`/`cargo-audit` gate yet. Dependabot security updates and
  gitleaks are enabled, but advisory/license policy is not enforced in
  CI.
- No coverage percentage, fuzzing or Miri gate yet. The sandbox and
  redaction tests are adversarial, but not exhaustive.
- GitHub reports secret-scanning non-provider patterns and validity
  checks as disabled for this repository.
- Judge calibration has no real human-labelled dataset; the judge stays
  advisory and is never promoted.
- The live watcher needs a read-only GitHub token in SSM; until then the
  service correctly stays disabled.

## Rules for new changes

- Add a test for behavior changes; update the nearest contract doc.
- Do not add a dependency without a concrete need.
- Do not weaken a gate to make a change pass.
- Keep Python stdlib-only, ruff-clean, under 100 columns, and importable.
- Keep generated files generated: `protocol/eval-protocol-v1.schema.json`
  is produced by `cargo run -p eval-protocol --example generate_schema`.
- Never commit secrets or run live-provider tests in the default path.
