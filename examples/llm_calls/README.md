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
- `target.py` — `eval/v1` adapter (`task`/`text`/`provider` params).
- `extract.yaml`, `classify.yaml`, `summarize.yaml` — offline suites.
- `live_smoke.yaml` — opt-in live smoke test (shape-only graders).
