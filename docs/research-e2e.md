# Research agents: real E2E

Live end-to-end evaluation of `research-orchestration-agents`.

Three lanes, deliberately separate:

| Lane | What it runs | What it proves |
|---|---|---|
| `live_service_local` | real target FastAPI app started with uvicorn on loopback | service entrypoint, job queue, renderer; target OTel + proxy usage captured |
| `live_service_remote` | deployed Render service | production service availability, queue/E2E latency, artifact/sources |
| `live_engine` (debug) | in-process `ResearchEngine` | stage-level code debugging only; not the canonical E2E lane |

The service lanes use one HTTP client (`service_runner.py`). The target repo is
never modified. The deterministic cassette suite remains the CI gate; none of
the live lanes run in CI or on PRs.

## Service contract

```
GET  /health                         -> {"status":"ok","engine":"ready"}
GET  /readiness                      -> {"ok":bool,"checks":{...}}
POST /research                       -> 202 {"task_id","status":"queued"}
GET  /result/{task_id}               -> queued|running|done|error
GET  /result/{task_id}/artifact      -> markdown
GET  /result/{task_id}/sources       -> {"task_id","thread_id","sources"}
```

Local readiness uses local stores, so `database`/`storage` are expected false;
the client requires `providers` and `auth` and records the raw checks. Remote
requires `readiness.ok == true`.

`datasets/research_e2e/topics.json` holds the topic, query, host allowlist,
source/scrape ceilings, and expected properties (`min_sources`,
`min_artifact_chars`, `min_citations`, forbidden text).

Cases:

| Case | What it proves |
|---|---|
| `research-service-health` | health/readiness only; no model call; safe `eval check --probe` |
| `research-service-e2e` | submit/poll/artifact/sources against local or Render |

## Service matrix

| Service | Implementation | Credential | Egress |
|---|---|---|---|
| Local target | `uvicorn knowledge_engine.service.api:app`, memory jobs, local stores | ephemeral per-run `API_TOKEN` | loopback |
| Local LLM | target `OpenRouterAdapter` -> eval-proxy -> OpenRouter free model | `OPENROUTER_API_KEY` from env/`.env`/OpenCode auth; target sees `dummy-eval-proxy` | proxy only |
| Remote target | Render `research-orchestration-agents.onrender.com` | app bearer `API_TOKEN` (`EVAL_RESEARCH_SERVICE_TOKEN` or `RENDER_API_KEY`); `RENDER_EVAL_API_KEY` (`rnd_...`) is a Render management key and is **not** accepted | HTTPS |
| Search/scrape | target defaults (`_default_search`, `ScrapeRouter`) | keyless scholarly APIs; optional `JINA_API_KEY` | scholarly hosts |
| Trace local | target OTel -> collector | none | `127.0.0.1:4328` / Phoenix `6006` |
| Store | `/var/lib/eval-runs` -> S3 | instance role | `s3://eval-store-342940030437-eu-north-1/eval-runs` |

Credentials and key resolution:

- Local free model: `key_resolver.py` checks `OPENROUTER_API_KEY` in the
  environment, then `EVAL_OPENROUTER_ENV_FILE`, then `EVAL_ENV_FILE`
  (`$ROOT/.env` by default), then the local OpenCode auth store's
  `openrouter` entry. The value is passed to the proxy process only.
- Remote app auth: `EVAL_RESEARCH_SERVICE_TOKEN` is the app's `API_TOKEN`.
  The local eval-service `.env` also has `RENDER_API_KEY` (43 chars, app token)
  and `RENDER_EVAL_API_KEY` (`rnd_...`, Render management key). Use
  `RENDER_API_KEY` or set `EVAL_RESEARCH_SERVICE_TOKEN`; the `rnd_` key is
  **not** accepted by `POST /research`.
- Health/readiness need no token; the remote script can produce availability
  evidence even before the app token is supplied.

The local target venv needs the project's `scrape` extra for arXiv HTML
(`trafilatura`; `crawl4ai`/Playwright optional). The target's `OPENROUTER_API_BASE`
is supported by litellm and is the seam that pins local model calls to
eval-proxy.

## Service E2E (canonical)

Local, with a free OpenRouter model through eval-proxy:

```bash
export EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
export EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
export EVAL_OPENROUTER_ENV_FILE=/path/to/.env   # contains OPENROUTER_API_KEY
export EVAL_FREE_MODEL=nvidia/nemotron-3-ultra-550b-a55b:free
export EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328
export EVAL_S3_URI=s3://eval-store-342940030437-eu-north-1/eval-runs

integrations/research_agents/run_live_service.sh local
```

The script starts a dedicated eval-proxy with model overrides to the free
model plus `reasoning_shim.py` (disables provider reasoning so synthesis does
not return empty content). The adapter starts the target uvicorn app in a
fresh temporary working directory **for each trial**, waits for `/health`,
runs the service suite, checks every proxy event's served model against the
free allowlist, tears everything down, and syncs to S3. No target code is
imported; the per-trial cwd prevents the target wiki cache from answering a
later trial.

Remote:

```bash
export EVAL_RESEARCH_SERVICE_URL=https://research-orchestration-agents.onrender.com
export EVAL_ENV_FILE=/path/to/eval-service/.env   # contains RENDER_API_KEY
# or: export EVAL_RESEARCH_SERVICE_TOKEN=...
export EVAL_S3_URI=s3://eval-store-342940030437-eu-north-1/eval-runs

integrations/research_agents/run_live_service.sh remote
```

From Windows, the remote health/readiness smoke test needs no token:

```powershell
$env:EVAL_RESEARCH_SERVICE_URL = "https://research-orchestration-agents.onrender.com"
$env:EVAL_RESEARCH_SERVICE_LANE = "remote"
$env:EVAL_PYTHON = "py"
.\target\debug\eval.exe check integrations/research_agents/live_service.yaml --probe
```

A full remote run additionally needs `EVAL_RESEARCH_SERVICE_TOKEN` (the app
`API_TOKEN`). Put it in the launching environment or a local `.env`; never
paste it into the suite or docs.

The remote script runs the health/readiness probe first and stops if the
service is not ready. It never queries Supabase; evidence is the eval run store
and S3.

Model policy: local calls are pinned to `EVAL_FREE_MODEL`; the proxy records
`requested_model`/`served_model` per attempt. If `EVAL_RESEARCH_REQUIRE_FREE_MODELS=1`
(default for local), the run fails when any served model is outside
`EVAL_FREE_MODEL_ALLOWLIST`. Tokens/cost are taken from provider-reported usage
and stay `n/a` when absent. Remote model selection is deployment-controlled;
remote internal tokens/cost/spans are `n/a`.

## `live_engine` debug lane (not canonical)

From the runner, as a user that can write the store (normally `eval`):

```bash
export EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
export EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
export EVAL_RESEARCH_MODEL=openai/gpt-4o-mini
export EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328
export EVAL_S3_URI=s3://eval-store-342940030437-eu-north-1/eval-runs

integrations/research_agents/run_live_e2e.sh
```

Trace context follows the shared convention: `eval run` creates the run trace,
writes `trace.json`, and injects `TRACEPARENT` / `EVAL_TRACE_ID` into the
adapter. The adapter forwards that context to the target, so target spans join
`eval.run` instead of starting a second trace. If invoked outside `eval run`,
the adapter also accepts `EVAL_RESEARCH_TRACEPARENT` / `EVAL_TRACEPARENT` /
`TRACEPARENT`. The OTLP endpoint comes from `EVAL_RESEARCH_TRACE_ENDPOINT`,
`EVAL_OTEL_ENDPOINT`, `EVAL_TRACE_OTLP_ENDPOINT` or
`OTEL_EXPORTER_OTLP_ENDPOINT` (explicit suite params win); a base URL is
normalized to `/v1/traces`.

The script:

- refuses to run when `CI` is set;
- takes an exclusive `flock` (`/tmp/eval-research-live.lock`);
- reads the provider key from SSM into a run-owned proxy only when it has to
  start one (never prints it); the shared proxy holds its own key;
- starts `eval proxy` on `127.0.0.1:8796` with
  `--max-requests 80 --max-cost-usd 0.25` and gpt-4o-mini prices; if the port
  is already occupied it fails instead of reusing unknown caps (set
  `EVAL_REUSE_PROXY=1` to reuse a shared proxy deliberately);
- runs one worker and stores under `/var/lib/eval-runs` with
  `--origin live`; the CLI owns the trace, and target spans join it;
- writes generated artifacts to `/var/lib/eval-runs/research-e2e-artifacts/`;
- syncs the store to S3 when `EVAL_S3_URI` is set.

The script defaults to `EVAL_PROXY_PORT=8796` and reuses the shared research
proxy (`EVAL_REUSE_PROXY=1`). The proxy enforces its cap; the suite
additionally enforces per-trial token/model-call limits. Set
`EVAL_REUSE_PROXY=0` to require a run-owned proxy.

Equivalent explicit command (the script is a wrapper around this):

```bash
sudo -u eval env \
  EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python \
  EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents \
  EVAL_RESEARCH_ENDPOINT=http://127.0.0.1:8796/v1 \
  EVAL_RESEARCH_MODEL=openai/gpt-4o-mini \
  EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328 \
  /srv/eval-service/target/release/eval run \
    integrations/research_agents/live_e2e.yaml \
    --ref research-e2e-live --origin live --workers 1 \
    --store /var/lib/eval-runs
```

Start the proxy by hand (key stays in the environment):

```bash
export OPENROUTER_API_KEY="$(aws ssm get-parameter \
  --name /eval/openrouter_api_key --with-decryption \
  --query Parameter.Value --output text)"
eval proxy --port 8796 --key-env OPENROUTER_API_KEY \
  --project research-agents --tag research-e2e \
  --store /var/lib/eval-runs --provider openrouter \
  --max-requests 80 --max-cost-usd 0.25 \
  --price-in-per-1m 0.15 --price-out-per-1m 0.60 \
  --upstream-timeout-s 180 https://openrouter.ai/api/v1
```

## Replay (offline, CI gate)

```bash
export EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
export EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
eval check integrations/research_agents/suite.yaml --probe
eval run integrations/research_agents/suite.yaml --ref baseline --set-baseline
eval run integrations/research_agents/suite.yaml --ref work
eval compare --suite integrations/research_agents/suite.yaml
```

`live_suite.yaml` is the middle ground: live model calls through the proxy,
retrieval replayed from the target's cassettes. It is still not CI; keep it as
a bounded smoke test.

Never present replay as live retrieval. The proxy stamps `origin: live` only
for a real proxy run; `eval cassette` replay stamps `origin: replay`.

## Recording

Target cassettes (LLM + search + scrape) live in the target repo under
`evals/cassettes/<case>/run0/`. This integration does **not** write them and
never edits the target. To re-record, use a disposable clone, wipe the case
directory, record, replay immediately, review, then copy the fixtures through
the target's own PR process:

```bash
git clone git@github.com:amagarzogonzalo/research-orchestration-agents.git /tmp/roa-record
cd /tmp/roa-record
rm -rf evals/cassettes/research/run0
EVAL_MODE=record python evals/runner.py --record -k research
python evals/runner.py -k research --json   # must reproduce bit-identically
```

Proxy cassettes (LLM responses only) use `eval proxy --cassette-dir DIR`;
replay them with `eval cassette DIR --port 8788` and point the target at that
loopback URL. Replay events are labeled `origin=replay`.

## Observed service-lane evidence (2026-10-21)

Remote Render health/readiness, no model call:

- `GET /health` 200 in 480ms; `GET /readiness` 200 in 2,292ms.
- checks: `database=true, storage=true, providers=true, auth=true,
  migrations=true`; `effective_ready=true`.
- A later cold-ish probe took 12.9s for readiness while health stayed 279ms;
  keep the per-request timings and do not label the cause without evidence.

Remote full service case:

- First attempt with `RENDER_EVAL_API_KEY` (`rnd_...`) returned **401**; it is a
  Render management key, not the app token. Stored run
  `1792570071051-d7ccce1e` failed closed with `service_auth`.
- Retry with `RENDER_API_KEY` (the app token in the eval-service `.env`)
  authenticated and passed **3/3** (`1792578953599-e398eb60`):
  - tasks `3648fe7c89654edc94f90faa39cfea45`,
    `781c4cc5cbee42d0b31ec6e58478e2f9`.
  - artifacts 16,817 / 16,874 chars; citations 7 / 7; sources 60 / 60
    (from artifact provenance; the `/sources` endpoint still returns 0).
  - E2E 33.5s / 1,010.7s; queue wait 18.5s / 65.8s; 2 / 100 polls.
  - health/readiness checks all true; submit 202 in ~3.6s.
- The raw `/artifact` response is still a wiki/source wrapper with multiple
  `## Article` blocks and an empty `/sources` payload. The client unwraps the
  **last** article, counts its frontmatter citation list and provenance
  entries, and records `sources_origin=artifact.provenance`,
  `citations_origin=frontmatter.citations`, plus raw and normalized hashes.
  This is a documented normalization of the service's own artifact, not
  fabricated evidence. The target should still return the rendered article and
  persisted sources directly.
- Earlier run `1792572582502-797b08f7` failed only because this normalization
  was not yet in place.

Free-model probe:

- Direct OpenRouter call to `nvidia/nemotron-3-ultra-550b-a55b:free` returned
  `{"ok":true}` with provider-reported usage 211 in / 17 out and cost 0.
- `minimax/minimax-m3.1-flash:free` returned JSON on a short probe but produced
  no synthesis body in a full local run; it is not the default. The
  `nvidia/nemotron-3-ultra-550b-a55b:free` run below produced the article.
- Through eval-proxy with `--override gpt-4o-mini=nvidia/nemotron-3-ultra-550b-a55b:free`,
  the response was served by the free model, usage was recorded (211 in / 6 out,
  cost 0), and the event carried `requested_model=gpt-4o-mini` plus
  `model=nvidia/nemotron-3-ultra-550b-a55b:free`.
- This is the default `EVAL_FREE_MODEL`; local proxy overrides pin every
  target request to it and the run is rejected if a non-free model is served.

Local service lane on Windows: **3/3 pass** (`1792577496709-e162fb99`).
The two full trials produced 16,531 / 14,979 char artifacts with 9 / 10
citations and 77 / 67 sources; E2E 139.2s / 109.7s, queue wait ~2.0s, free
model verified, usage 6 / 5 calls. The mock suite
(`1792570715652-e2cd468b`) also passed 3/3.

Local fixes discovered by running on the user's machine:

- the target venv needed the `scrape` extra (`trafilatura`); without it all
  publisher/arXiv extraction failed and the renderer had no bodies;
- the target service must run in a **fresh temporary working directory per
  trial**; otherwise its wiki cache answers the second trial and skips model
  calls;
- free reasoning models can spend the whole `max_tokens` budget on reasoning
  and return an empty `content`. `reasoning_shim.py` adds
  `reasoning={"enabled": false}` before eval-proxy; the proxy still records
  the request/usage and the free-model allowlist still applies;
- pin `nvidia/nemotron-3-ultra-550b-a55b:free`; `minimax` returned JSON on a
  short probe but produced no synthesis body in a full run.

The local suite is now runnable with:
`python integrations/research_agents/run_live_service.py --mode local
--target-repo <target> --target-python <target-venv-python>`.

## Observed live_engine debug result (2026-10-20)


- Run: `1792529562412-a34ad760`, produced by `run_live_e2e.sh` against the
  shared proxy and collector; ref `research-e2e-live`, suite
  `research-agents-e2e`, origin `live`, workers `1`.
- Result: **3/3 trials passed** (research 2/2, guardrail 1/1), pass rate 1.0.
- Topic: `crispr-offtarget` (`biomed` invariants), model `openai/gpt-4o-mini`.
- Retrieval per research trial: 29 sources found, 3 relevance-filtered, 3 real
  scrapes succeeded from `arxiv.org`, 1 search call, 3 bodies, 3 citations.
- Artifact per trial: 5,863 / 5,902 chars; SHA-256
  `26845408...1731c` / `6cdc39de...8e8e8`.
- Wall: 42.6s and 46.2s (research), 11.0s (guardrail, no network).
- Proxy: 8 calls, 8,902 input / 2,828 output tokens; estimated **$0.00303** at
  $0.15/$0.60 per 1M; p50 3,122ms, p95 23,220ms; 8/8 ok.
- Trace: `00000000000031c1b04791e3e0d0aea0` (the CLI's `eval.run` trace; the
  target joined it via `TRACEPARENT`). Phoenix shows 70 spans: 1 `eval.run`,
  2 `eval.case`, 3 `eval.trial`, 3 `eval.research_e2e`, 8 `llm.call`, 2
  `eval.retrieval.search`, 6 `eval.retrieval.scrape`, 3 `eval.grade`, plus
  `node.*`, `search.federated` and `scrape.ScrapeRouter`.
- Stored locally and synced to
  `s3://eval-store-342940030437-eu-north-1/eval-runs/runs/research-agents-e2e/research-e2e-live/run-1792529562412-a34ad760/`.
- Artifacts:
  `/var/lib/eval-runs/research-e2e-artifacts/00000000000031c1b04791e3e0d0aea0-ae1bde.md`
  and `...-5e91d7.md`.
- Offline replay re-ran green (`run 1792529765070-ce6504a5`).

Trace UI (runner loopback; tunnel from your workstation):

```bash
ssh -i ~/.ssh/eval-runner-aws \
  -L 6006:127.0.0.1:6006 \
  ubuntu@13.60.60.51
# http://localhost:6006  -> trace 00000000000031c1b04791e3e0d0aea0
```

## Good and bad examples

Good (real result):

```
PASS crispr-offtarget live (5863 chars, 29 sources, 3 bodies)
trace=00000000000031c1b04791e3e0d0aea0
```

Bad (retrieval seam absent): the first RAG topic run scraped zero bodies
because the venv had no local extractor and Jina returned 403 for arXiv. The
suite failed closed:

```
failures: ["expected_bodies: 0", "expected_live_scrape: 0"]
```

Bad (invariant regression): running the CRISPR topic against `research.yaml`'s
`min_citations: 4` produced `only 3 citations, need >= 4`. The biomed case
keeps its calibrated `min_citations: 3`; the point is that the gate notices.

Bad (egress guardrail): `research-e2e-allowlist-blocked` requests
`https://malicious.example/...` and returns:

```
status: blocked, code: host_not_allowlisted, network_calls: 0
```

Bad (wrong service token): `RENDER_EVAL_API_KEY` (`rnd_...`) submitted to
`POST /research` returned 401. The client classified it as `service_auth`,
stopped before polling, and wrote no artifact. Use the app token
`RENDER_API_KEY` or `EVAL_RESEARCH_SERVICE_TOKEN`.

Bad (service artifact): the same job with the app token completed but the API
returned 0 sources and an article body with empty citation markers. The
grader failed with `too few citations` / `too few sources`; it did not
fabricate either. The client now unwraps the last `## Article` block and reads
the frontmatter provenance/citations with explicit origins, which makes the
remote lane pass; the target should still return a clean article + sources.

Bad (service not ready): if `/health` is up but `/readiness` fails, the client
stops before submit and reports `service_not_ready`; it never spends a model
call on a service that cannot serve.

## Scheduling and operations


The live paths run on the runner host, not in the proxy-only Docker sandbox.
The local in-process debug unit is `eval-live-research.service` /
`eval-live-research.timer` (files under `infra/aws/systemd/`, `make live-research`).

The service-entrypoint lane needs a separate pair so a Render outage or auth
failure cannot block the local lane:

```ini
# infra/aws/systemd/eval-live-research-service.service
[Unit]
Description=eval-service research service E2E (local or Render)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=eval
Group=eval
WorkingDirectory=/srv/eval-service
Environment=EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
Environment=EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
Environment=EVAL_OPENROUTER_ENV_FILE=/etc/eval/research-openrouter.env
Environment=EVAL_FREE_MODEL=nvidia/nemotron-3-ultra-550b-a55b:free
Environment=EVAL_OTEL_ENDPOINT=http://127.0.0.1:4328
Environment=EVAL_S3_URI=s3://eval-store-342940030437-eu-north-1/eval-runs
ExecStart=/srv/eval-service/integrations/research_agents/run_live_service.sh local
TimeoutStartSec=1800
Nice=10
```

For Render, use the same unit with `remote` and
`Environment=EVAL_RESEARCH_SERVICE_URL=...` plus a root-readable
`EVAL_RESEARCH_SERVICE_TOKEN` (app `API_TOKEN`, not a Render `rnd_` key).
The suggested service timer is `05:10 UTC`; Agent A owns the unit and
`make live-research-service` target.

Manual triggers:

```bash
sudo systemctl start eval-live-research.service
sudo systemctl start eval-live-research-service.service
journalctl -u eval-live-research-service.service -n 80 --no-pager
```

The service must not run in PR CI. The PR gate stays on `suite.yaml`
(`origin=replay`). Keep `WorkingDirectory`/`EVAL_BIN` on the canonical checkout.

## Budget and safety rules

- Local service lane: dedicated free-model proxy on `127.0.0.1:8797`, request
  cap 80, free model `nvidia/nemotron-3-ultra-550b-a55b:free`; the suite timeout is
  1,200s per trial. No cost cap is claimed for free models; usage comes from
  provider-reported tokens.
- `live_engine` debug lane: proxy `127.0.0.1:8796`, request/cost caps as before.
- Remote service lane: Render owns its provider budget; eval-service records
  only service-level latency/status and marks internal tokens/cost `n/a`.
- Overlap: `run_live_service.sh` and `run_live_e2e.sh` hold exclusive `flock`
  locks and refuse to run under `CI`.
- Secrets: local OpenRouter/HuggingFace/Google keys are read from the
  environment or a configured env file by `key_resolver.py`; the target sees a
  dummy key. The Render app token is `EVAL_RESEARCH_SERVICE_TOKEN` (the app's
  `API_TOKEN`), not the Render `rnd_` management key. No key is written to the
  store, cassettes, spans, logs, or docs.
- Page contents are not emitted. Spans carry host names, char counts, token
  counts, and artifact hashes. Generated artifacts are written outside git
  under the eval store for inspection.
- Trace linkage: the target service does not extract `traceparent`, so its
  internal spans may form a separate trace. Correlate with
  `task_id`/`thread_id`, `service.run_tag`, and timestamps; do not claim
  parent-child linkage.
