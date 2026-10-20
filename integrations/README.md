# Real-project integrations (zero changes to the projects)

Every integration is a thin adapter + suite that lives **here**, never in
the project under test. The engine stays generic: adding a target needs
no engine changes, and no target repository is modified.

Start with the tooling: `eval schema` (grader kinds, fields, protocol,
exit codes), `eval init --template cli|llm|agent` (runnable scaffold),
and `eval check <suite> --probe` (the deterministic gate). Full contract:
`docs/integration.md`.

## Catalogue

| Integration | What it exercises | Suite | Network / keys |
|---|---|---|---|
| `cv_toolkit` | deterministic CLI contract | `suite.yaml` | none |
| `digest_agent` offline | real LangGraph orchestrator + the project's own fakes | `suite.yaml` | none |
| `digest_agent` live | real LLM summarization through `eval proxy` | `live_bench.yaml` | proxy only |
| `research_context` | deterministic `bin/context_budget.py` CLI | `suite.yaml` | none |
| `research_agents` replay | the target's own cassette replay harness | `suite.yaml` | none |
| `research_agents` live | real research workflow + live LLM, recorded retrieval | `live_suite.yaml` | proxy only |

`research_context` and `research_agents` are different seams on the same
project: the first grades a deterministic budget-report CLI, the second
runs the real research workflow.

## Machine configuration (no file edits)

Committed suites contain `$VAR` references, never absolute paths; the
adapters expand them and fail with a named `bad_params` error when one
is missing.

| Variable | Used by |
|---|---|
| `EVAL_CV_PYTHON`, `EVAL_CV_SRC`, `EVAL_CV_REPO` | `cv_toolkit` |
| `EVAL_DIGEST_PYTHON`, `EVAL_DIGEST_SRC` | `digest_agent` offline and live |
| `EVAL_DIGEST_LIVE=1`, `EVAL_DIGEST_API_KEY`, `EVAL_DIGEST_ENDPOINT`, `EVAL_DIGEST_MODEL` | `digest_agent` live (key is a dummy; the proxy holds the real one) |
| `EVAL_RES_PYTHON`, `EVAL_RES_REPO` | `research_context` |
| `EVAL_RESEARCH_PYTHON`, `EVAL_RESEARCH_REPO` | `research_agents` replay/live |
| `EVAL_RESEARCH_ENDPOINT`, `EVAL_RESEARCH_MODEL` | `research_agents` live (loopback proxy) |

## Run examples

```sh
# deterministic, offline
EVAL_CV_PYTHON=… EVAL_CV_SRC=… EVAL_CV_REPO=… \
  eval run integrations/cv_toolkit/suite.yaml --ref work

EVAL_DIGEST_PYTHON=… EVAL_DIGEST_SRC=… \
  eval run integrations/digest_agent/suite.yaml --ref work

EVAL_RES_PYTHON=… EVAL_RES_REPO=… \
  eval run integrations/research_context/suite.yaml --ref work

EVAL_RESEARCH_PYTHON=… EVAL_RESEARCH_REPO=… \
  eval check integrations/research_agents/suite.yaml --probe
```

Live suites require an explicit opt-in and a running `eval proxy`:

```sh
EVAL_DIGEST_LIVE=1 EVAL_DIGEST_API_KEY=dummy-eval-proxy-observes \
EVAL_DIGEST_ENDPOINT=http://127.0.0.1:8793/v1 \
EVAL_DIGEST_MODEL=openai/gpt-4o-mini \
EVAL_DIGEST_PYTHON=… EVAL_DIGEST_SRC=… \
  eval run integrations/digest_agent/live_bench.yaml --workers 1

EVAL_RESEARCH_PYTHON=… EVAL_RESEARCH_REPO=… \
EVAL_RESEARCH_ENDPOINT=http://127.0.0.1:8795/v1 \
EVAL_RESEARCH_MODEL=openai/gpt-4o-mini \
  eval run integrations/research_agents/live_suite.yaml
```

Live suites are **never** run in CI. The proxy is the only key holder and
the only usage observer; the target receives a dummy key. On the 2 GiB
runner, `openai/gpt-4o-mini` is the reliable live model; reasoning models
(`gpt-oss-20b`) produced 107–114 s calls that exceeded the target's own
timeout and are recorded honestly as failures.

## What this proves

Adding a project required no engine changes and no project changes: an
adapter plus a suite, with the target's own contract as the source of
truth. The engine owns execution, measurement, comparison, and reporting
only.
