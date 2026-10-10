# Contributing

Small, testable steps. This repository values demonstrated behavior
over claims: every material change needs a test and, where it affects
the user, a docs touch.

## Workflow

1. Read the relevant doc in `docs/` first (architecture decisions in
   `docs/02-architecture.md` and `docs/08-decisions.md` are binding
   unless renegotiated).
2. Add or extend tests *before* changing behavior (TDD). Unit tests
   live next to each crate in `tests/`; end-to-end coverage lives in
   `adapters/rust_shim/tests/`.
3. Extend — don't rewrite. The 5-crate split and the `eval/v1`
   protocol are stable; additive changes only (see `docs/03-protocol-v1.md`
   compatibility policy).
4. Validate: `cargo test --workspace`, `cargo fmt --all -- --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`.
   Python changes: keep `adapters/` and `examples/` stdlib-only and
   under 100 columns (`ruff check`, `py_compile` in CI).

## What good changes look like

- New graders: config-driven spec in `eval-core`, evaluation in
  `eval-graders`, adversarial tests (absent fields must fail, bad
  patterns must fail loudly).
- Statistics: hand-rolled, with unit tests against independently
  computed reference values. Never present a heuristic as significance.
- Failure taxonomy (`ok / case_error / timeout / crash /
  protocol_error`) is never merged. A handled `case_error` is graded;
  the rest are observations with kill+respawn recovery.
- Secrets: never capture env contents, never store key material.
  Redaction has property tests — extend them with new shapes.
- No network in the default test path. Live/external calls are
  explicit opt-ins with budgets, documented in the relevant README.

## Scope discipline

The v1 exclusion list (`docs/05-mvp.md`: server, dashboard backend,
distributed execution, plugin ABI, SQLite, MCP, agent harness) and
the Phase-2 non-goals still hold. Each addition needs a tested use
case. License: MIT; keep new files header-free but the workspace
`license = "MIT"` intact.
