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

## Live recording is deliberately absent

The target's `--record` writes its cassettes and calls providers with its
own keys. That path must go through `eval proxy` as the single observer
and a temp copy of the target; it is a separate Phase 3/2 step, not part
of the offline integration.
