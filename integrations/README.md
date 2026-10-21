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
| `digest_agent` offline | real LangGraph orchestrator + the project's own fakes | `suite.yaml` | none |
| `digest_agent` live | real entrypoint + free Groq models + PDF/SMTP | `live_e2e.yaml` | GitHub + free Groq |
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
| `EVAL_DIGEST_PYTHON`, `EVAL_DIGEST_SRC` | `digest_agent` offline and live |
| `EVAL_DIGEST_GROQ_API_KEY` (free tier), `EVAL_DIGEST_GITHUB_TOKEN` | `digest_agent` live entrypoint (free-only) |
| `EVAL_RES_PYTHON`, `EVAL_RES_REPO` | `research_context` |
| `EVAL_RESEARCH_PYTHON`, `EVAL_RESEARCH_REPO` | `research_agents` replay/live |
| `EVAL_RESEARCH_ENDPOINT`, `EVAL_RESEARCH_MODEL` | `research_agents` live (loopback proxy) |

## Run examples

```sh
# deterministic, offline
EVAL_DIGEST_PYTHON=… EVAL_DIGEST_SRC=… \
  eval run integrations/digest_agent/suite.yaml --ref work

EVAL_RES_PYTHON=… EVAL_RES_REPO=… \
  eval run integrations/research_context/suite.yaml --ref work

EVAL_RESEARCH_PYTHON=… EVAL_RESEARCH_REPO=… \
  eval check integrations/research_agents/suite.yaml --probe
```

Live suites require an explicit opt-in and a running `eval proxy`:

```sh
EVAL_DIGEST_GROQ_API_KEY=… EVAL_DIGEST_GITHUB_TOKEN=… \
  bash integrations/digest_agent/run_e2e.sh live

EVAL_RESEARCH_PYTHON=… EVAL_RESEARCH_REPO=… \
EVAL_RESEARCH_ENDPOINT=http://127.0.0.1:8795/v1 \
EVAL_RESEARCH_MODEL=openai/gpt-4o-mini \
  eval run integrations/research_agents/live_suite.yaml
```

Live suites are **never** run in CI. Digest model selection is restricted
to the target's free Groq list and fails closed on rate limits; research
live runs remain proxy-observed.

## What this proves

Adding a project required no engine changes and no project changes: an
adapter plus a suite, with the target's own contract as the source of
truth. The engine owns execution, measurement, comparison, and reporting
only.
