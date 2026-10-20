# digest_e2e dataset

Inputs and expected properties for the real `digest_agent` E2E suites
(`integrations/digest_agent/live_e2e.yaml` and
`live_e2e_replay.yaml`). See `docs/digest-e2e.md` for the service matrix,
credentials, budget, observed runs, and exact commands.

## Files

| File | Kind | Provenance |
|---|---|---|
| `github_activity.json` | recorded GitHub activity | Real `GET /repos/amagarzogonzalo/eval-service/pulls/10` and `/files` (2026-10-20), patches truncated to 1,200 chars. Public data; no credentials. |
| `llm_fixture.json` | synthetic OpenAI-compatible response | Hand-written deterministic fixture for offline replay. **Not live evidence.** It returns a digest-like section with a `PASS` verdict so the real orchestrator can complete its review loop. |

## Cases and expected properties

`live_e2e.yaml` (live, never in CI):

1. **`e2e-fixture-smoke`** — network-free wiring check. Fixture activity,
   fake LLM, **real ReportLab PDF**, **real SMTP delivery**. Expects
   `outcome=success`, one PDF with `%PDF` magic, exactly one SMTP
   message with a PDF attachment.
2. **`e2e-github-live-success`** — the real positive case. Fetches real
   GitHub activity for `amagarzogonzalo/eval-service` (7-day window,
   pruned to the 3 most recent PRs), calls a real model through
   `eval proxy`, renders a real PDF, sends a real SMTP message. Two
   trials (live output is sampled); expects structural facts only:
   at least one PR and one file, a PDF, one email with a PDF, a trace
   id, and no credential patterns in the digest text.
3. **`e2e-github-live-bad-repo`** — the real negative case. Fetches a
   repository that does not exist; expects `outcome=error`,
   `GitHub API error`, no email, no PDF. This proves the GitHub call is
   real and failures are handled without side effects.

`live_e2e_replay.yaml` (offline, safe for CI):

- **`e2e-fixture-replay`** — recorded activity + synthetic LLM fixture
  (or a cassette when `EVAL_REPLAY_MODE=cassette`). Real orchestrator,
  real PDF, real SMTP sink. Expects `PR #10`,
  `crates/eval-cli/src/watch.rs`, a PDF, one email with a PDF, and a
  trace id. `origin=replay`.

## Recorded activity facts

From real PR #10, `Watch: recover stale state locks after SIGTERM`:

- merged 2026-10-20; +101/−14 across 3 files;
- `crates/eval-cli/src/watch.rs` (+96/−12);
- `docs/watch.md` (+4/−2);
- `infra/aws/systemd/eval-watch.service` (+1/−0).

The deterministic summary in the target always includes these facts, so
the grader can assert them without depending on model wording.

## Refreshing the GitHub fixture

Use a read-only token; never write it to a file or the repository:

```sh
TOKEN=$(aws ssm get-parameter --name /eval/github_watch_token \
  --with-decryption --query Parameter.Value --output text)
curl -s -H "Authorization: Bearer $TOKEN" \
  https://api.github.com/repos/amagarzogonzalo/eval-service/pulls/10
```

Keep the shape used by `driver.py` (`repos[].pull_requests[]` and
`commits[]`, `FileChange` fields). Truncate patches; do not record diffs
from private repositories or any credential.

## Budget and cadence

- Live: one `eval proxy` run per invocation, `--workers 1`,
  `--max-requests 200`, `--max-cost-usd 0.20`, `openai/gpt-4o-mini`
  at 0.15/0.60 USD per 1M input/output tokens. Expected cost
  approximately USD 0.05-0.12 per run (two live trials).
- Timer: daily at 04:10 UTC with up to 300 s jitter
  (`eval-live-digest.timer`); a `flock` prevents overlap.
- Replay: offline, no budget; expected runtime a few seconds.

**Known blocker (2026-10-20):** the OpenRouter key in
`/eval/openrouter_api_key` hit its USD 0.50 key limit (spent ~0.513),
so live calls return HTTP 403 until the key limit is raised or the key
is rotated. The suite fails honestly with `fail_exhausted`; replay is
unaffected.
