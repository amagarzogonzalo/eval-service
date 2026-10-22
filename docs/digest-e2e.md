# Digest agent real E2E (free-only entrypoint)

Production operator path: [`production.md`](production.md). Evidence
interpretation: [`benchmark-evidence.md`](benchmark-evidence.md).

This lane runs `digest_agent` exactly as its own entrypoint:

```sh
python -m digest_agent.main
```

in a temporary working directory. It uses real GitHub activity, the
target's real ReportLab renderer, and its real SMTP notifier. The only
configured model provider is the target's **Gemini free-tier adapter**
(Groq is an explicit optional fallback); there is no paid fallback. Inner stage/token metrics are `n/a` because
the target exposes none and no proxy is used.

The target repository is never modified.

## Run

```sh
# live (VM, after explicit approval; free-tier Gemini or Groq key)
sudo systemctl start eval-live-digest.service
make live-digest
sudo journalctl -u eval-live-digest.service -f

# direct, same code path
EVAL_DIGEST_GITHUB_TOKEN=<read-only token> \
  bash integrations/digest_agent/run_e2e.sh live

# offline replay (real entrypoint, fixture transports, no network)
bash integrations/digest_agent/run_e2e.sh replay
```

Suites: `integrations/digest_agent/live_e2e.yaml` and
`live_e2e_replay.yaml`. Fixtures: `datasets/digest_e2e/`.

## Free-only enforcement

The digest path fails the gate if any of the following is untrue:

1. `EVAL_DIGEST_PROVIDER` is `gemini` (default; free candidates
   `gemini-3.6-flash`, `gemini-flash-lite-latest`) or `groq` (free
   candidates `openai/gpt-oss-120b`, `openai/gpt-oss-20b`,
   `groq/compound-mini`).
2. Only the selected provider key (`GEMINI_API_KEY` or `GROQ_API_KEY`)
   reaches the entrypoint. `OPENROUTER_API_KEY`, `OPEN_ROUTER_API_KEY`,
   the other provider key, and `MISTRAL_API_KEY` are removed, so there is no non-free provider and no paid fallback. The
   entrypoint runs an isolated copy of the target `src` under the temp
   cwd, so a checkout `.env` (which the target's `load_dotenv()` would
   otherwise find) cannot inject provider keys.
3. `EVAL_DIGEST_ALLOW_PAID` is unset/`0`.
4. `EVAL_DIGEST_MODEL`, if set, is a free/fixture id.
5. `integrations/digest_agent/free_only.py` scans digest-owned suites,
   runner, docs, datasets, and the systemd unit for paid model ids.
6. The same script imports the target's real `build_llm_provider` with a
   fake provider transport and proves it exposes exactly the reviewed free
   candidates, can test the free model, and cannot route a paid id.

The replay transport additionally rejects any model outside the free
candidates. If free models are rate-limited or exhausted, the
target's model-selection loop returns `fail_exhausted`; the suite fails.
It never falls back to a paid model.

## Service matrix

| Service | Implementation | Configuration |
|---|---|---|
| Entrypoint | target `python -m digest_agent.main` | temporary cwd, `PYTHONPATH=src` |
| Activity | target `GitHubAdapter` (real REST) | `EVAL_DIGEST_GITHUB_TOKEN`, `EVAL_DIGEST_GITHUB_REPOS`, `EVAL_DIGEST_LOOKBACK_DAYS` |
| LLM | target `GeminiAdapter` (default) or `GroqAdapter` | selected key mapped from `EVAL_DIGEST_GEMINI_API_KEY`/`EVAL_DIGEST_GROQ_API_KEY`; all other provider keys removed |
| Renderer | target `ReportLabAdapter` | `DIGEST_OUTPUT_DIR` in the temp cwd |
| Notifier | target `SMTPAdapter` | `NOTIFIER=smtp`; local real SMTP test sink by default, external `SMTP_*` drop-in |
| Trace | adapter/entrypoint outer spans | `EVAL_OTEL_ENDPOINT`, `EVAL_TRACE_OUT` |

The local SMTP sink (`smtp_sink.py`) is a real STARTTLS/AUTH/DATA
exchange that persists `.eml` files and an `eval.mail/v1` index. No
external mail credential is provisioned; setting `SMTP_HOST` uses an
approved staging SMTP host instead.

Replay transport (`replay_sitecustomize.py`) is installed only in
`fixture` mode. It serves GitHub activity and the selected provider's
completions from the committed fixtures and raises on any unexpected network call.

## Credentials and systemd

No secret is stored in the repository.

| Name | Source |
|---|---|
| `EVAL_DIGEST_GEMINI_API_KEY` | free-tier Gemini key (default provider) |
| `EVAL_DIGEST_GROQ_API_KEY` | free-tier Groq key (optional provider) |
| `EVAL_DIGEST_PROVIDER_KEY_FILE` | fallback dotenv file; default `/srv/targets/digest_agent/.env`; only the selected provider key line is read |
| `EVAL_DIGEST_GITHUB_TOKEN` | read-only token env or `/eval/github_watch_token` |
| `SMTP_*` | optional approved staging SMTP |
| `EVAL_OTEL_ENDPOINT` | `http://127.0.0.1:4328` on the VM |

If the selected provider key is not exported, the runner reads **only**
that key's line (`GEMINI_API_KEY` or `GROQ_API_KEY`) from
`EVAL_DIGEST_PROVIDER_KEY_FILE` (the target's own `.env` by default). Other lines, including paid provider keys, are
never imported or logged. The target `.env` is not modified. The
free-only preflight includes a parser test proving this isolation.

The systemd unit also accepts an optional operator file:

```ini
EnvironmentFile=-/etc/eval/digest-provider.env
# file mode 0600, owned by eval; optional explicit provider key:
# EVAL_DIGEST_GEMINI_API_KEY=...
```

Timer: daily at **04:10 UTC** with up to 300 s jitter
(`eval-live-digest.timer`). One worker and an exclusive
`/var/lib/eval-runs/.digest-e2e.lock` prevent overlap. S3 sync is on for
live runs.

## Output metrics

`entrypoint_runner.py` writes `digest.output-metrics/v1` to per-case
`metrics-<case_id>.json` under the output directory and as the case
`output.data`. Observed fields:

- `run_id`, `trace_id`, `started_at`, `finished_at`, `wall_ms`
- `exit_status`, `outcome`, `failure_class`
- `pdf_path`, `pdf_bytes`, `pdf_sha256`, `pdf_ok`
- `email_sent`, `notifier_receipts`, `notifier_has_pdf`
- `request_counts` (`observed` + GitHub/model counts in fixture mode;
  n/a for live)
- `free_only`, `paid_model_configured`, `provider`
- `inner_metrics.stages/tokens/cost = n/a`

The JSON is also graded by the suites (exit code, outcome, PDF, email,
free-only flags, fixture request counts). Inner tokens/cost are never
fabricated.

## Observed replay metrics (free-only)

The replay suite runs the real entrypoint with fixture transports:

- Run: `1792572597437-9e3d2d76`, trace
  `00000000000031c1fe8f50a90440390c`
- Outcome: `success`; exit `0`; wall **1,392 ms**
- PDF: real ReportLab file, `%PDF`, **7,283 bytes**, SHA-256
  `a1f6dff6…cda2f8`
- Email: one SMTP message with the PDF attachment
- Requests: **5 GitHub calls, 14 Gemini completions**, 0 paid attempts,
  model `gemini-3.6-flash`
- Inner metrics: `n/a`
- Published record: `datasets/digest_e2e/output_metrics_replay.json`

## Observed live metrics (free-only, 2026-10-21)

The committed live suite ran the real entrypoint against real GitHub,
the free Gemini provider, the real PDF renderer, and the real SMTP sink:

- Run: `1792572765100-f6f8a5ff`, trace
  `00000000000031c1fedd63af6db054cc`; 4 cases / 5 trials, **1.00**
- Live success trials: **141,634 ms** and **106,324 ms**; exit `0`;
  outcome `success`
- PDFs: **78,185 bytes** (`ce051de4?088b44`) and **76,082 bytes**
  (`22413781?b9443b`), both `%PDF`
- Email: one SMTP message with the PDF attachment per trial
- `free_only=true`, `paid_model_configured=false`, provider `gemini`
- Request counts: **n/a** (live path has no proxy); inner stages, tokens,
  and cost: **n/a**
- Published record: `datasets/digest_e2e/output_metrics_live_gemini.json`

The fail-closed and bad-repo cases also passed: no key -> 
`no_llm_provider`, real 404 -> `github_error`, no PDF/email. The earlier
paid-model runs are **historical plumbing evidence only** and are **not
canonical**: do not cite their cost/quality, and do not run them.

## Failure behavior

- Empty provider configuration: the real entrypoint exits nonzero with
  `No LLM provider configured`; the fail-closed case records
  `failure_class=no_llm_provider`.
- Missing live credentials: metrics record `missing_credentials`; no
  process is started.
- Bad GitHub repo: real 404 or fixture 404 -> `outcome=error`,
  `failure_class=github_error`, no PDF, no email.
- Free model rate limit/exhaustion: `fail_exhausted`; no paid fallback.
- SMTP/render failures: classified `smtp_error`/`render_error`; missing
  artifacts fail the structural graders.

## Replay

```sh
bash integrations/digest_agent/run_e2e.sh replay

# direct
EVAL_DIGEST_PYTHON=... EVAL_DIGEST_SRC=... \
EVAL_DIGEST_E2E_FIXTURE=datasets/digest_e2e/github_activity.json \
EVAL_DIGEST_LLM_FIXTURE=datasets/digest_e2e/llm_fixture.json \
  eval run integrations/digest_agent/live_e2e_replay.yaml \
    --origin replay --workers 1
```

Replay is deterministic, offline, and serves `origin=replay`; it is
never live evidence. `eval check --probe` runs the free-only preflight
first, so it spends no model calls.

## Trace and artifacts

- Run record: `/var/lib/eval-runs/runs/<suite>/<ref>/run-<id>/`
- PDFs and SMTP inbox:
  `/var/lib/eval-runs/e2e-artifacts/digest-e2e-<mode>-<stamp>/`
- Span JSONL: `/var/lib/eval-runs/e2e-traces/digest-e2e-<mode>-<stamp>.jsonl`
- Phoenix: `http://127.0.0.1:6006` via SSH tunnel; OTLP/HTTP
  `http://127.0.0.1:4328`
- S3: `s3://eval-store-342940030437-eu-north-1/eval-runs/`

Traces are observability only and never decide the gate.
