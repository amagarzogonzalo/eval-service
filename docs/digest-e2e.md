# Digest agent real E2E

This document covers the real end-to-end evaluation of `digest_agent`:
real GitHub activity, a real model through `eval proxy`, the target's
real ReportLab renderer, and its real SMTP notifier. The target
repository is never modified.

The repeatable entry points are:

```sh
# live (VM; SSM credentials + eval-proxy)
sudo systemctl start eval-live-digest.service
sudo journalctl -u eval-live-digest.service -f
make live-digest

# offline replay (CI/dev; no network)
bash integrations/digest_agent/run_e2e.sh replay

# direct live run (same code path as the timer)
bash integrations/digest_agent/run_e2e.sh live
```

Suites: `integrations/digest_agent/live_e2e.yaml` (live) and
`integrations/digest_agent/live_e2e_replay.yaml` (replay). Inputs and
expected properties: `datasets/digest_e2e/`.

## Service matrix

| Service | Real implementation | How it is enabled | What is exercised |
|---|---|---|---|
| GitHub activity | target `github_adapter.GitHubAdapter` | `EVAL_DIGEST_GITHUB_TOKEN`, `EVAL_DIGEST_GITHUB_REPOS`, `EVAL_DIGEST_LOOKBACK_DAYS`, `EVAL_DIGEST_MAX_PRS` | `https://api.github.com` commit/PR/diff fetch, real pagination and 404s |
| LLM | `integrations/digest_agent/live_provider.py` | `EVAL_DIGEST_LIVE=1`, `EVAL_DIGEST_API_KEY` (dummy), `EVAL_DIGEST_ENDPOINT` (proxy), `EVAL_DIGEST_MODEL` | OpenAI-compatible calls through `eval proxy`; provider-reported tokens |
| Renderer | target `reportlab_adapter.ReportLabAdapter` | `real_services: true` (suite) | real multi-page PDF, hashed and checked for `%PDF` |
| Notifier | target `gmail_adapter.SMTPAdapter` | `real_services: true`; `SMTP_*` | real STARTTLS + AUTH LOGIN + MIME/PDF delivery |
| Trace | adapter/driver spans + engine trace spine | `EVAL_OTEL_ENDPOINT`, `EVAL_TRACE_OUT` | OTLP/HTTP to the collector; JSONL evidence; W3C `traceparent` |

### Notifier destination (explicit substitution)

No Slack or external email/staging credential is provisioned in SSM or
the runner environment. The target's real `SMTPAdapter` is therefore
pointed at `integrations/digest_agent/smtp_sink.py`, a real local SMTP
server (STARTTLS, AUTH LOGIN/PLAIN, DATA) that persists each accepted
message as `.eml` plus an `eval.mail/v1` index record. This is a real
protocol exchange against a test inbox, **not** a fake notifier; the
run fails if the sink is unavailable. When a staging SMTP credential is
approved, set `SMTP_HOST`/`SMTP_PORT`/`SMTP_USER`/`SMTP_PASSWORD` and
the adapter will not start the local sink. The target's `ResendAdapter`
is also wired via `EVAL_DIGEST_NOTIFIER=resend` and
`EVAL_DIGEST_RESEND_API_KEY` for a future approved key.

Replay uses a recorded GitHub activity fixture plus either a committed
synthetic LLM fixture (default) or an `eval cassette`. Neither replay
mode is live evidence.

## Credentials

Never commit or print any of these values.

| Name | Source | Used by |
|---|---|---|
| `/eval/openrouter_api_key` | SSM SecureString | `eval proxy` (`--key-env OPENROUTER_API_KEY`); the target sees only a dummy key |
| `/eval/github_watch_token` | SSM SecureString | real GitHub reads (`EVAL_DIGEST_GITHUB_TOKEN`); read-only |
| `EVAL_DIGEST_PYTHON` | runner env | `/srv/eval-venvs/digest-agent/bin/python` |
| `EVAL_DIGEST_SRC` | runner env | `/srv/targets/digest_agent/src` |
| `SMTP_*` | optional approved staging SMTP | external notifier; otherwise the local test sink |

The runner fetches the two SSM values when they are not already in the
environment. The proxy holds the real key in its process environment;
the target receives `dummy-eval-proxy-observes`. Run records, traces,
and artifacts are scanned for key patterns by the suite's `not_regex`
grader.

## Budget and cadence

`run_e2e.sh live` enforces:

- `--workers 1` and an exclusive `flock` on
  `/var/lib/eval-runs/.digest-e2e.lock` (overlapping runs fail fast);
- proxy `--max-requests 200` and `--max-cost-usd 0.20` with
  `0.15`/`0.60` USD per 1M input/output tokens;
- per-process `EVAL_DIGEST_MAX_REQUESTS=200`;
- `EVAL_DIGEST_MAX_PRS=3` to bound the GitHub window fed to the
  orchestrator (the fetch itself is real; fetched vs selected counts are
  recorded);
- adapter deadline 420 s and suite limit 480 s.

The systemd timer is daily at **04:10 UTC** with up to 300 s jitter
(`eval-live-digest.timer`). Expected spend at `openai/gpt-4o-mini` is
approximately **USD 0.05-0.12 per run** (two live trials). The observed
runs used 50 and 120 proxy calls respectively; the systemd run at the
time still used the older 120-request unit setting. The committed
runner defaults to 200 requests so a normal run is never clipped.

## Observed real runs (2026-10-20)

### Successful live run (integration trace)

- Run: `1792528045931-d21dcf41`, ref `live`, origin `live`
- Result: 3 cases / 4 trials, pass rate **1.00**
  - `e2e-github-live-success`: 2/2 live trials
  - `e2e-github-live-bad-repo`: real GitHub 404 handled as `error`
  - `e2e-fixture-smoke`: real renderer + real SMTP, no model call
- GitHub: real `amagarzogonzalo/eval-service` window (13 PRs fetched)
- Proxy: 120 events, 120 ok / 0 error / 0 timeout / 0 retries
- Tokens: **636,296 in / 25,081 out** (provider-reported)
- Latency: p50 **1,782 ms**, p95 **12,399 ms**, max 18,379 ms
- Estimated cost: **USD 0.1105** (pricing table absent; computed from
  the proxy event counts with the 0.15/0.60 rate card)
- Side effects: 3 PDFs (6,680 / 60,211 / 60,595 bytes), 3 SMTP
  messages with PDF attachments, 0 messages for the bad-repo case
- Trace JSONL: 163 spans / 4 traces; re-exported to the OTel collector.
  Trace IDs: `e038b446148c4a2dce2a25e7dfa08e82` (smoke),
  `c9fa7a2d9f22688d81c868e2c6ad2928` and
  `e367fa3deb067dc5c5420e56b76cb355` (live trials),
  `ff37f32e769dd5390b4120ee4208865d` (bad repo). This run predates the
  engine trace spine; its traces are integration spans.

### Systemd live run (engine + integration trace)

- Run: `1792531414244-87b35627`, launched by
  `eval-live-digest.service`; trace
  `00000000000031c1b3a5e58d1794c02b`
- Result: 3 cases / 4 trials, pass rate **1.00**; live trials
  89.2 s and 49.9 s
- GitHub: same real repo window (14 PRs, 47 commits, 146 files)
- Proxy: 50 events, 44 ok / 6 handled errors (provider/key limit);
  303,786 in / 12,451 out; estimated **USD 0.0530**
- Side effects: 3 PDFs (84,180 / 82,149 / 6,680 bytes: two live
  trials plus the smoke case), 3 SMTP messages with PDF attachments
- Phoenix: 101 spans for the trace, including `eval.run`, `eval.case`,
  `eval.trial`, `digest.activity.fetch`, `digest.llm.*`,
  `digest.render`, `digest.notify`, `digest.grade.evidence` (renamed
  `digest.grade` after this run)
- S3: run directory, `trace.json`, trace JSONL, PDFs, and mailbox
  index synced under `s3://eval-store-342940030437-eu-north-1/eval-runs/`

The systemd unit at the time ran the deployed `/srv/eval-service` copy.
After this branch merges, the unit should run the committed runner; the
required environment additions are listed below.

### Replay run

- Run: `1792533468772-1a6dcbf1`, ref `replay`, origin `replay`
- Trace: `00000000000031c1b7629c59ab8cd495`
- Result: 1 case / 1 trial, pass rate **1.00**; real renderer, real
  SMTP sink, fixture activity, synthetic LLM fixture; no network
- Artifacts: `github_digest_2026-10-20_6C541514.pdf` (7,282 bytes) and
  one `.eml` in the mailbox index
- Lock check: a second invocation while the lock was held exited with
  `another digest E2E run holds /var/lib/eval-runs/.digest-e2e.lock`

## Good and bad examples

Good (live): the success case fetches real activity, the model names a
real PR (`PR #<n>`) and real changed files, a valid PDF is rendered, and
exactly one SMTP message with the PDF attachment lands in the test
inbox. The grader asserts properties, never exact wording.

Good (replay): the fixture contains PR #10 of `eval-service`
(`crates/eval-cli/src/watch.rs`, `docs/watch.md`,
`infra/aws/systemd/eval-watch.service`); the replay must reference
`PR #10` and the watch file, render a PDF, and deliver it.

Bad (real negative): `EVAL_DIGEST_BAD_REPOS` defaults to a repository
that does not exist. The target makes a real GitHub API call, receives
404, returns `outcome: error` with `GitHub API error`, sends no email,
and writes no PDF. The suite fails if any side effect occurs.

Bad (honest budget failure): a run on 2026-10-20 at 21:27 UTC hit
OpenRouter `HTTP 403` on the first live test call. The suite failed with
`fail_exhausted` for both trials; it did **not** fall back to a fake
model. This is recorded as a real failure, not a pass.

## Current blocker: provider key limit

The OpenRouter key in `/eval/openrouter_api_key` has a **USD 0.50 key
limit** and was already at **USD 0.5129 spent** (remaining negative) on
2026-10-20. Further live calls return `HTTP 403`. No reset interval is
configured, so live runs (manual or timer) will fail until the account
owner raises the key limit, rotates the key, or provisions a new key in
SSM. Replay is unaffected. The 21:27 run above is the honest failure
evidence; the earlier runs in this document are the live evidence.

## Exact live command

The runner is the single command used by the systemd unit and by hand:

```sh
sudo -u eval env \
  EVAL_MODEL=openai/gpt-4o-mini \
  EVAL_MAX_PRS=3 \
  EVAL_S3_SYNC=1 \
  EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328 \
  bash /srv/eval-service-digest-live/integrations/digest_agent/run_e2e.sh live
```

It resolves the two SSM credentials when absent, starts
`eval proxy` on `127.0.0.1:8793` with a cassette directory, runs the
suite with `--workers 1 --origin live --store /var/lib/eval-runs`,
writes PDFs/mailboxes/traces under the store, and syncs to S3.

The deployed systemd unit should set the same environment. In
particular, the unit needs `EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328`
(the engine trace spine), `EVAL_MAX_PRS=3`, `EVAL_MAX_REQUESTS=200`, and
`EVAL_S3_SYNC=1`.

## Replay instructions

```sh
# deterministic, offline (GitHub fixture + synthetic LLM fixture)
bash integrations/digest_agent/run_e2e.sh replay

# same path through the engine directly
EVAL_DIGEST_PYTHON=... EVAL_DIGEST_SRC=... \
EVAL_DIGEST_E2E_FIXTURE=datasets/digest_e2e/github_activity.json \
EVAL_DIGEST_LIVE=1 EVAL_DIGEST_API_KEY=dummy-eval-proxy-observes \
EVAL_DIGEST_MODEL=openai/gpt-4o-mini \
EVAL_DIGEST_MOCK_FILE=datasets/digest_e2e/llm_fixture.json \
  eval run integrations/digest_agent/live_e2e_replay.yaml \
    --origin replay --workers 1
```

For a cassette instead of the synthetic fixture:

```sh
# record once, when a live provider budget is available
EVAL_... eval proxy --key-env OPENROUTER_API_KEY \
  --cassette-dir /var/lib/eval-runs/cassettes/digest-e2e \
  ... https://openrouter.ai/api/v1
# ... run live_e2e_replay.yaml once with the fixture activity and that proxy

# replay offline
EVAL_REPLAY_MODE=cassette EVAL_CASSETTE_DIR=/var/lib/eval-runs/cassettes/digest-e2e \
  bash integrations/digest_agent/run_e2e.sh replay
```

A cassette is keyed by the exact request body, so the activity fixture
and model id must match the recording. `eval cassette` serves unmatched
requests as 404 `cassette_miss`; it never guesses.

## Trace and artifact locations

- Run record: `/var/lib/eval-runs/runs/<suite>/<ref>/run-<id>/`
  (`metadata.json`, `observations.jsonl`, `grades.json`,
  `trace.json`)
- PDFs and SMTP inbox:
  `/var/lib/eval-runs/e2e-artifacts/digest-e2e-<mode>-<stamp>/`
- Span JSONL:
  `/var/lib/eval-runs/e2e-traces/digest-e2e-<mode>-<stamp>.jsonl`
- Phoenix UI: `http://127.0.0.1:6006` (SSH tunnel); OTLP/HTTP
  collector `http://127.0.0.1:4328`
- S3: `s3://eval-store-342940030437-eu-north-1/eval-runs/`

Traces do not decide the gate. A collector outage is logged and never
changes a run result.

