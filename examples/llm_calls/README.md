# LLM-call examples: evaluating an ordinary LLM-enabled app

Three small, realistic tasks — structured extraction, constrained
classification, coverage-graded summarization — behind one
provider-neutral interface (`provider.py`):

- `FakeProvider` (default): deterministic fixtures. No network, no keys.
- `LiveProvider`: OpenAI-compatible `chat/completions` over HTTPS,
  mirroring the sibling repositories' `openai_compat` pattern
  (Bearer auth, `{endpoint}/chat/completions`). Explicit opt-in only.

## What is measured (and what is not)

| Example | Measures | Does not measure |
|---|---|---|
| `extract.yaml` | JSON shape (`name`+`email`), email format | Semantic correctness of names, live-model wording |
| `classify.yaml` | Output constrained to the label set (+ exact label offline) | Whether the label is *right* beyond the fixture |
| `summarize.yaml` | Structure, keyword coverage, length/citation heuristic (`judge_lite`) | True summary quality — `judge_lite` is a stand-in, documented as such |

The offline suites may use `exact` graders (the fake is deterministic).
Live runs must never be gated on exact text — `live_smoke.yaml` uses
shape-only graders for this reason.

## Offline (default, CI)

```sh
eval run examples/llm_calls/extract.yaml --ref work
eval run examples/llm_calls/classify.yaml --ref work
eval run examples/llm_calls/summarize.yaml --ref work
```

No keys, no network. The fake emits fixed fixtures; the adapter omits
the `ai` block (absent = n/a, never zero).

## Live (explicit opt-in, never in CI)

```sh
EVAL_LLM_LIVE=1 EVAL_LLM_API_KEY=<key> \
  eval run examples/llm_calls/live_smoke.yaml --ref live
```

Optional: `EVAL_LLM_ENDPOINT` (default `https://api.openai.com/v1`),
`EVAL_LLM_MODEL` (default `gpt-4o-mini`),
`EVAL_LLM_MAX_REQUESTS` (default 5 per process),
`EVAL_LLM_TIMEOUT_S` (default 30). Without `EVAL_LLM_LIVE=1` the
target returns `case_error: live_not_enabled` — it never silently
falls back to (or up to) a live provider, in either direction.

Safety rules, all enforced:

- Credentials come exclusively from environment variables and are
  never written to suites, traces, run files, or logs (the engine's
  redaction drops `Authorization`/Bearer shapes as a second net).
- The `ai` block records model + token usage *when reported*; absent
  fields stay absent.
- Live output varies between runs: smoke runs are inspected, not
  gated against a baseline.
- Small smoke test (3 requests), bounded time, no retries, no
  paid benchmarking.

## Files

- `provider.py` — provider interface + fake/live implementations.
- `target.py` — `eval/v1` adapter (`task`/`text`/`provider`/`variant` params).
- `extract.yaml`, `classify.yaml`, `summarize.yaml` — offline suites.
- `benchmark_a.yaml`, `benchmark_b.yaml` — fixture benchmark variants.
- `live_smoke.yaml` — opt-in live smoke test (shape-only graders).

## Benchmark: comparing two variants (fixture demo)

Same tasks, same graders, two fixture personalities (`a`: concise,
`b`: verbose with citations). Everything downstream of fixture text
is synthetic demonstration — it proves the quality/cost/latency
machinery, never that one real model beats another.

```sh
EVAL_USAGE_TAG=bench-a EVAL_USAGE_OUT=a.jsonl \
  eval run examples/llm_calls/benchmark_a.yaml --ref work --store ./store
# run id printed as: run: <run-a> (ref work)
EVAL_USAGE_TAG=bench-b EVAL_USAGE_OUT=b.jsonl \
  eval run examples/llm_calls/benchmark_b.yaml --ref work --store ./store
eval usage-import a.jsonl --project llm-examples --store ./store \
  --tag bench-a --suite examples/llm_calls/benchmark_a.yaml --ref work
eval usage-import b.jsonl --project llm-examples --store ./store \
  --tag bench-b --suite examples/llm_calls/benchmark_b.yaml --ref work
eval usage-report --project llm-examples --store ./store \
  --pricing pricing/prices.toml
```

The dashboard's benchmark-comparison table joins each tag's usage
(cost, latency, tokens) with its bound run's mean `judge_lite`
quality score. Expected shape: variant B scores higher and costs
more (richer fixture text under illustrative pricing); latencies are
both local-instant and reported as measured. Parallel runs need no
coordination: usage files use region-locked appends so concurrent
writers never lose events.

## Live smoke-test operator procedure

Authorized operator only, strict budget, never in CI:

1. Confirm no `EVAL_LLM_API_KEY` is stored anywhere except your shell.
2. Run, replacing the key:
   ```sh
   EVAL_LLM_LIVE=1 EVAL_LLM_API_KEY=<key> \
     eval run examples/llm_calls/live_smoke.yaml --ref live --store ./store
   ```
   Defaults bound the run: 3 requests, 60 s per case, no retries,
   model `gpt-4o-mini` (override via `EVAL_LLM_MODEL`,
   `EVAL_LLM_ENDPOINT`, `EVAL_LLM_MAX_REQUESTS`, `EVAL_LLM_TIMEOUT_S`).
3. Inspect: transport success (exit code), schema validity (grades),
   task correctness (read the outputs — an HTTP 200 is not proof),
   and variability (rerun once; outputs will differ).
4. Check the stored run for secrets:
   `grep -ri "sk-\|bearer" <store>/runs/llm-live-smoke/` must be empty.
5. Report: model, token usage from the `ai` blocks, grades, and cost.
   Until this procedure has genuinely run, live inference is
   **unverified** — the mock-transport tests prove the pipeline,
   not the provider.
