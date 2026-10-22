# Testing

This page maps every test layer to its local command and its CI job. The
browser suite uses only synthetic demo data; it is not production evidence.

## Test layers

| Layer | What it proves | Command |
|---|---|---|
| Rust | Engine, protocol, runners, graders, stores, CLI, offline examples | `make quality` (or the raw cargo gates in `AGENTS.md`) |
| Python unit/API | Private service sessions, path safety, run summaries, judge helpers, UI contracts | `make python-test` |
| Store tools | `scripts/verify_store.py` and `scripts/retention.py` against temporary stores | included in `make python-test` |
| Browser E2E | Token login, run detail, sources/answer, judge, benchmarks, filters, legacy toggle, no console errors | `cd e2e && npm test` |
| Python lint | ruff + `compileall` for adapters, examples, integrations, web and scripts | `make python-lint` |
| Dependency policy | RustSec advisories, licenses, duplicate/wildcard policy, registries | `cargo deny check` |
| PR dependency review | New PR dependencies at high/critical severity | GitHub Actions only |

The E2E tests live in `e2e/`. They do not replace the Python API tests and
do not call any live service.

## Browser E2E

### Prerequisites

- Python 3.11+ (the repository's normal prerequisite)
- Node.js `22.19.0` (pinned in the CI workflow and `e2e/package.json`)
- npm, using the committed `e2e/package-lock.json`

Install once:

```sh
cd e2e
npm ci
npx playwright install chromium
```

Run:

```sh
cd e2e
npm test
```

`npm test` runs the Playwright global setup, which:

1. Creates a temporary store with `scripts/seed_demo_store.py`.
2. Adds two clearly synthetic fixture runs in that temporary store: one
   current run with a different query and numeric metrics, and one legacy
   run without a manifest. They exist to exercise filters and the legacy
   toggle; they are not committed run data and are not production evidence.
3. Starts `web/private_service.py` on `http://127.0.0.1:8091` with the
   test-only token `demo`, signing key `demo-only`, and
   `--allow-http-cookie`.
4. Waits for `/healthz`, runs the tests, then stops the server and removes
   the temporary store.

Useful overrides:

| Variable | Purpose |
|---|---|
| `EVAL_E2E_PORT` | Change the loopback port (default `8091`) |
| `EVAL_E2E_PYTHON` | Select the Python interpreter (default: `python3`, `python`, then `py -3`) |
| `EVAL_E2E_KEEP=1` | Keep the temporary store for debugging |
| `CI=true` | Enable one retry, HTML report, and CI reporter behavior |

On failure, Playwright keeps a screenshot, trace and video under
`e2e/test-results/`. Run `npx playwright show-report` to inspect the HTML
report. In CI the same files are uploaded as the
`playwright-private-ui` artifact.

The suite covers:

- unauthenticated page redirect and `/api/*` 401 responses,
- token login and Overview,
- Runs table → demo run,
- Query / Sources / Answer with source cards and the synthetic RAG answer,
- Judge overall score, five dimension bars, rationale, and issues,
- four benchmark charts, query and model filters, model cards, and the
  disaggregated table,
- RAG query filtering of table rows, model cards, and chart points,
- legacy toggle behavior while the demo run stays current,
- browser console errors and uncaught page errors.

The console guard ignores only a missing `/favicon.ico`; the private service
does not serve one.

Do not point the E2E setup at a production store or production credentials.
The token and signing key above are deliberately fake and are used only for
the loopback test server.

## Store tool tests

`web/test_store_scripts.py` is discovered by the existing
`python3 -m unittest discover -s web -p "test_*.py"` command. Every test
uses `tempfile.TemporaryDirectory`; no S3, AWS CLI, VM, or restore script is
invoked.

It covers:

- `verify_store.py`: missing store, empty store, invalid/non-object
  metadata, missing metadata, invalid manifest, valid run acceptance, and
  CLI exit codes.
- `retention.py`: dry-run planning without deletion, `--apply` deletion,
  newest-N protection per suite/ref group, the age cutoff, invalid
  arguments, and missing stores. Directory mtimes are set explicitly so the
  tests do not depend on wall-clock timing.

Restore behavior is intentionally out of scope. The tests never run
`scripts/restore_drill.sh` or contact AWS.

## CI structure

`ci-fast.yml` remains the main gate:

- Rust on Ubuntu and Windows (`make verify` / raw cargo commands),
- ARM64 `make verify`,
- MSRV 1.85,
- offline test run with a dead proxy,
- `make python-lint`,
- `make python-test`,
- gitleaks over full git history.

`e2e-private-ui.yml` runs on every push and pull request:

- Ubuntu, Python 3.12, Node `22.19.0`,
- `npm ci` from the committed lockfile,
- `npx playwright install --with-deps chromium`,
- `npm test` against the synthetic loopback demo store,
- failure-only upload of `e2e/playwright-report/` and
  `e2e/test-results/`.

It has `contents: read` and no secrets, AWS, LLM keys, or self-hosted
runner.

`security-dependencies.yml` adds:

- `cargo-deny` 0.20.2 (installed from crates.io with `--locked`) on pull
  requests, pushes to `main`, and a weekly schedule,
- `actions/dependency-review-action` on pull requests, failing at
  `high` severity and leaving PR comments disabled (`pull-requests: read`).

The existing `pr-gate`, Terraform, and `live-production` workflows are
unchanged.

## Dependency policy scope

`deny.toml` configures:

- advisories from the public RustSec database, with yanked crates denied,
- an SPDX allowlist for the resolved dependency tree,
- warnings for duplicate versions and workspace-internal wildcard path
  dependencies,
- crates.io as the only allowed registry and no allowed git sources.

This is **dependency and vulnerability scanning, not a formal security
audit**. It does not inspect application logic, run CodeQL, scan containers,
or replace the gitleaks secret scan. No paid account or personal token is
required.

Known policy notes:

- `RUSTSEC-2024-0449` (`unsafe-libyaml` is unmaintained) is ignored with a
  reason because it comes from the existing `serde_yaml` dependency and has
  no safe upgrade. Replacing that parser is a separate change; the ignore
  is not a claim that the crate is safe.
- `rustls-webpki` and `webpki-roots` were updated in `Cargo.lock` to the
  patched versions required by `RUSTSEC-2026-0382` and
  `RUSTSEC-2026-0381`.
- Duplicate-version and internal wildcard findings are warnings, not
  failures, so the job remains actionable without blocking unrelated
  dependency work. External wildcard requirements remain visible.

## Known gaps

- Browser coverage is Chromium only; Firefox and WebKit are not tested.
- E2E uses synthetic fixtures and does not prove live-model, AWS, VM, or
  production behavior.
- No coverage percentage, fuzzing, Miri, CodeQL, container scan, or visual
  regression gate.
- Dependency review depends on GitHub's dependency graph; if GitHub cannot
  generate the diff, the job reports that instead of inventing a result.
