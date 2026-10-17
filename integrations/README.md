# Real-project integrations (zero changes to the projects)

Three genuinely different projects evaluated through the same engine,
each via a thin adapter (stdlib Python only) that lives here — never in
the project under test. Adding one needs no engine changes.

**Start with the tooling, not this file:** `eval schema` (grader kinds,
field paths, protocol, exit codes), `eval init --template cli|llm|agent`
(runnable scaffold), `eval check <suite> --probe` (deterministic gate).
Full contract: `docs/integration.md`.

## Machine configuration (no file edits needed)

Committed suite files contain `$VAR` references, never absolute
paths. Set these once per machine; the adapters expand them and
fail with a named `bad_params` error when one is missing:

| Variable | Example | Used by |
|---|---|---|
| `EVAL_CV_PYTHON` | `…/other/cv_toolkit/.venv/Scripts/python.exe` | cv_toolkit adapter |
| `EVAL_CV_SRC` | `…/other/cv_toolkit/src` | cv_toolkit adapter (`PYTHONPATH`) |
| `EVAL_CV_REPO` | `…/other/cv_toolkit` | cv_toolkit adapter (child cwd) |
| `EVAL_DIGEST_PYTHON` | `…/digest_agent/.venv/Scripts/python.exe` | digest_agent adapter |
| `EVAL_DIGEST_SRC` | `…/digest_agent/src` | digest_agent adapter (`PYTHONPATH`) |
| `EVAL_RES_PYTHON` | `…/research-orchestration-agents/.venv/Scripts/python.exe` | research_context (child command) |
| `EVAL_RES_REPO` | `…/research-orchestration-agents` | research_context (child cwd) |

```sh
EVAL_CV_PYTHON=… EVAL_CV_SRC=… EVAL_CV_REPO=… \
  eval run integrations/cv_toolkit/suite.yaml --ref work
EVAL_DIGEST_PYTHON=… EVAL_DIGEST_SRC=… \
  eval run integrations/digest_agent/suite.yaml --ref work
EVAL_RES_PYTHON=… EVAL_RES_REPO=… \
  eval run integrations/research_context/suite.yaml --ref work
```

## research_context — scaffolded CLI wrapper (zero adapter edits)

The third integration was onboarded with **only** the agent-facing
surface: `eval init research_context --template cli`, then a suite. No
adapter edits — the template's `command`/`cwd` params carry everything
project-specific. It wraps `research-orchestration-agents/bin/context_budget.py`
(exit 0, a budget report on stdout) and grades objective properties:
exit code, the `TOTAL n of m budget` line, `AGENTS.md` presence,
latency bound, and byte-stability of the report across two runs.
This is the pattern to copy for any deterministic CLI.

## cv_toolkit — deterministic CLI

`adapter.py` shells out to the project's own venv python
(`PYTHONPATH` → its `src/`) and relays the real exit contract:
exit 0 carries stdout as `output.text`; nonzero exits become this
adapter's exit code (a `Crash` observation carrying the code), so
suites grade the project's actual contract with `exit_code` graders.

`suite.yaml` covers: good resume validates (exit 0 + schema message),
stable rerun (trials 2 + `stability`), missing file rejected (exit 1).

Run: `eval run integrations/cv_toolkit/suite.yaml --ref work`
(needs that repo's venv; adjust the absolute paths in the suite file).

## digest_agent — agent orchestrator, offline

`driver.py` wires the project's **real LangGraph orchestrator** to its
**own fakes** (`FakeActivitySource`, `FakeLLMProvider`,
`FakeDocumentRenderer`, `FakeNotifier`) and prints the final digest
state as JSON. `adapter.py` relays it into `eval/v1`. No network, no
keys, no PDF, no email.

`suite.yaml` covers: empty window → "Nothing to report today" with a
bounded attempt count; single merged PR → deterministic snapshot with
real diff data, bounded loops, stable reruns.

Run: `eval run integrations/digest_agent/suite.yaml --ref work`
(needs that repo's venv; adjust the absolute paths in the suite file).

## What this proves

Adding a project required no engine changes and no project changes:
an adapter script plus a suite file. The engine never learned
anything about CVs or digests — it owns execution, measurement,
comparison, and reporting only.
