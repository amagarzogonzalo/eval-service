# research-orchestration-agents (cassette replay)

This integration runs the target's **own** cassette harness:

```
target/evals/runner.py -k research --json
```

The target records every LLM/search/scrape boundary to
`evals/cassettes/`; replay is offline, deterministic, and needs no
provider keys. eval-service grades the target harness result
(`ok`, token/call budget, artifact size, source count), so the target's
invariants remain the gate and eval-service remains the observer.

## Setup (runner)

```bash
git clone git@github.com:amagarzogonzalo/research-orchestration-agents.git \
  /srv/targets/research-orchestration-agents
# install the target environment OUTSIDE the checkout
curl -LsSf https://astral.sh/uv/install.sh | sh
cd /srv/targets/research-orchestration-agents
UV_PROJECT_ENVIRONMENT=/srv/eval-venvs/research-agents \
  ~/.local/bin/uv sync --frozen --no-dev
```

The adapter runs the target in a temporary working directory with
`PYTHONDONTWRITEBYTECODE=1`; cassettes and snapshots are read-only.

## Run

```bash
export EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
export EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
eval check integrations/research_agents/suite.yaml --probe
eval run integrations/research_agents/suite.yaml --ref baseline --set-baseline
eval run integrations/research_agents/suite.yaml --ref work
eval compare --suite integrations/research_agents/suite.yaml
```

## Live LLM (recorded retrieval)

`live_suite.yaml` runs the real research workflow with **live model calls
through eval proxy**. Search/scrape stay pinned to the target's cassettes
(bounded evidence); the proxy is the only key holder and the only usage
observer:

```bash
export EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
export EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
export EVAL_RESEARCH_ENDPOINT=http://127.0.0.1:8795/v1
export EVAL_RESEARCH_MODEL=openai/gpt-4o-mini
eval check integrations/research_agents/live_suite.yaml --probe
eval run integrations/research_agents/live_suite.yaml --ref research-live
eval usage-report --project digest-agent --tag research-live
```

The target process receives only a dummy key; the real key lives in the
proxy. The live runner writes only to a temporary cwd. Two trials are
required because live model output is nondeterministic; the target's
invariants decide pass/fail.

Live **recording** of the target's own search/scrape cassettes is still a
separate action (`evals/runner.py --record`), deliberately not part of
this integration because it writes the target's evaluation fixtures.

