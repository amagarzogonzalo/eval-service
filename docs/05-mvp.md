# E. MVP specification

## Included in v1

1. Runnable Rust CLI (`eval run|compare|report|init`), clap-based,
   `--json` machine output alongside human output.
2. Versioned subprocess protocol (`eval/v1`, worker + oneshot) per
   `docs/03-protocol-v1.md`, schema JSON generated and committed.
3. Python reference target + Rust reference target, schema-equal on
   the same fixture (conformance-proven).
4. Deterministic case format (suite YAML + JSONL cases, versioned).
5. Configurable graders: exit_code, exact, regex, json_schema,
   range, stability, file_exists.
6. Structured run results + file-based local persistence per
   `docs/04-domain-model.md` (raw/derived separation, redaction).
7. Baseline comparison (`compare --baseline <ref> --candidate <ref>`)
   with deterministic diff first.
8. **Intentional-regression demo:** a committed fixture change that
   flips the gate to exit 1 with a written explanation (what changed,
   which cases, what evidence).
9. Timeouts + recovery: hang/crash/malformed conformance targets in
   the default offline test run.
10. Offline end-to-end tests: full `cargo nextest` suite makes zero
    network calls (judge tests use a fake judge; live judge is an
    explicit opt-in command).
11. Integration docs: tutorial taking a new project from zero to
    gated suite, with the `cv_toolkit` adapter as the worked example.
12. Two-project demo: `cv_toolkit` CLI suite (deterministic) and
    `digest_agent` fakes suite (bounded nondeterminism) through the
    same engine, same report format.

## Explicitly excluded from v1

Server, dashboard, distributed execution, hosted service; plugin ABI;
SQLite (file store + migration trigger instead); MCP; agent harness
(separate, optional, later); PDF rendering; live-provider evals in
the default path; sandboxing claims beyond §C's honest boundary.

## Acceptance tests (measurable)

- **A1 (thin adapter):** a developer unfamiliar with the engine
  integrates a third CLI tool with an adapter of ≤ ~100 lines, a
  suite YAML, and **zero engine code changes**; reviewer verifies by
  `git diff --stat` on `crates/` (empty).
- **A2 (gate fires):** the intentional-regression fixture produces
  exit code 1, names the changed case(s), and cites evidence lines.
- **A3 (gate stays quiet):** re-running an unchanged suite twice
  yields exit 0 with identical digests.
- **A4 (recovery):** hang target → `Timeout`, crash target →
  `Crash`, malformed target → `ProtocolError`, each with no worker
  cross-contamination (subsequent cases still pass).
- **A5 (parity):** Python and Rust shims produce schema-equal runs on
  the shared fixture (byte-compare after `seq`-independent
  normalization).
- **A6 (offline):** default test run with networking disabled passes
  (CI enforces via a no-network job or proxy block).
- **A7 (two-project demo):** one `eval report` renders both the
  `cv_toolkit` and `digest_agent` suites with correct
  deterministic-vs-sampling handling per case.
