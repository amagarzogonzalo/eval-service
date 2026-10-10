# Real-project integrations (zero changes to the projects)

Two genuinely different projects evaluated through the same engine,
each via a thin adapter (~60–90 lines of Python, stdlib only) that
lives here — never in the project under test.

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

```sh
EVAL_CV_PYTHON=… EVAL_CV_SRC=… EVAL_CV_REPO=… \
  eval run integrations/cv_toolkit/suite.yaml --ref work
EVAL_DIGEST_PYTHON=… EVAL_DIGEST_SRC=… \
  eval run integrations/digest_agent/suite.yaml --ref work
```

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
