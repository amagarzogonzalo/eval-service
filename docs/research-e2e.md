# Research agents: real E2E

Live end-to-end evaluation of `research-orchestration-agents`: real scholarly
search, real page fetches, real model calls through `eval proxy`, the target's
own renderer, and trace evidence. The deterministic cassette suite remains the
CI gate; this suite never runs in CI or on PRs.

Owner: Agent C (integration + suite). Shared collector/proxy/timers: Agent A.

## What runs

```
datasets/research_e2e/topics.json
        │ query, allowlist, budgets, expected properties
        ▼
integrations/research_agents/live_e2e.yaml
        │ eval/v1
        ▼
adapter.py ──► live_runner.py (target venv)
        │             │
        │             ├─ live search  (OpenAlex, Crossref, PubMed,
        │             │                Europe PMC, Wikipedia, arXiv)
        │             ├─ live scrape  (target ScrapeRouter; host allowlist)
        │             ├─ live LLM     (LiteLLM -> eval proxy -> OpenRouter)
        │             ├─ renderer     (target RenderNode + invariants)
        │             └─ OTLP/HTTP    (W3C traceparent propagated)
        ▼
eval run ──► grades ──► /var/lib/eval-runs ──► S3
```

Cases:

| Case | What it proves |
|---|---|
| `research-live-e2e` | Live search + live scrape + live model + renderer, 2 trials |
| `research-e2e-allowlist-blocked` | A URL outside the allowlist is refused before any network call |

`datasets/research_e2e/topics.json` holds the topic, query, host allowlist,
source/scrape ceilings, and expected properties (`min_sources`, `min_bodies`,
`min_artifact_chars`, `min_citations`, token/call budgets, forbidden text).

## Service matrix

| Service | Implementation | Credential | Egress |
|---|---|---|---|
| LLM | `eval proxy` -> OpenRouter | `OPENROUTER_API_KEY` from SSM `/eval/openrouter_api_key`; target sees `dummy-eval-proxy` | proxy only |
| Search | target `DemuxSearchAdapter` (OpenAlex, Crossref, PubMed, Europe PMC, Wikipedia, arXiv) | none | `api.openalex.org`, `api.crossref.org`, `eutils.ncbi.nlm.nih.gov`, `www.ebi.ac.uk`, `en.wikipedia.org`, `export.arxiv.org`, `arxiv.org` |
| Scrape | target `ScrapeRouter` (ar5iv, Europe PMC fulltext, Crawl4AI, Trafilatura, Jina) | `JINA_API_KEY` optional; Jina works keyless but rate-limited | URLs must pass `allow_hosts` in the topic; currently `arxiv.org`, `ar5iv.org`, Europe PMC/PubMed, Wikipedia, ACL, OpenReview, PMLR, NeurIPS, bioRxiv, MDPI, PLOS, Frontiers |
| Render | target `RenderNode` | none | none |
| Trace | target `telemetry.span` -> OTLP/HTTP | none | collector (for example Jaeger on `127.0.0.1:4318`) |
| Store | `/var/lib/eval-runs` -> S3 | instance role | `s3://eval-store-342940030437-eu-north-1/eval-runs` |

The target venv needs the project's `scrape` extra for arXiv HTML (`trafilatura`;
`crawl4ai`/Playwright optional). Without it, local extraction is skipped and
only Jina remains; Jina currently returns 403 for arXiv PDFs, so bodies can be
zero. Build a dedicated live venv instead of mutating a shared one:

```bash
cd /srv/targets/research-orchestration-agents
UV_PROJECT_ENVIRONMENT=/srv/eval-venvs/research-agents-live \
  ~/.local/bin/uv sync --frozen --no-dev --extra scrape
```

The runner used for the evidence below has `trafilatura 2.3.1` installed in
`/srv/eval-venvs/research-agents`.

## Live run (manual)

From the runner, as a user that can write the store (normally `eval`):

```bash
export EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
export EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
export EVAL_RESEARCH_MODEL=openai/gpt-4o-mini
export EVAL_RESEARCH_TRACE_ENDPOINT=http://127.0.0.1:4318/v1/traces
export EVAL_S3_URI=s3://eval-store-342940030437-eu-north-1/eval-runs

integrations/research_agents/run_live_e2e.sh
```

The script:

- refuses to run when `CI` is set;
- takes an exclusive `flock` (`/tmp/eval-research-live.lock`);
- reads the provider key from SSM into the proxy process only (never prints it);
- starts `eval proxy` on `127.0.0.1:8796` if no proxy is already listening, with
  `--max-requests 80 --max-cost-usd 0.25` and gpt-4o-mini prices;
- generates a W3C `traceparent`, runs one worker, and stores under
  `/var/lib/eval-runs` with `--origin live`;
- writes generated artifacts to `/var/lib/eval-runs/research-e2e-artifacts/`;
- syncs the store to S3 when `EVAL_S3_URI` is set.

If Agent A's shared proxy is already running, set `EVAL_PROXY_PORT` to it and
the script reuses it. The proxy enforces the cap; the suite additionally
enforces per-trial token/model-call limits.

Equivalent explicit command (the script is a wrapper around this):

```bash
sudo -u eval env \
  EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python \
  EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents \
  EVAL_RESEARCH_ENDPOINT=http://127.0.0.1:8796/v1 \
  EVAL_RESEARCH_MODEL=openai/gpt-4o-mini \
  EVAL_RESEARCH_TRACE_ENDPOINT=http://127.0.0.1:4318/v1/traces \
  EVAL_RESEARCH_TRACEPARENT=00-<32hex>-<16hex>-01 \
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

## Observed real result (2026-10-20)

- Run: `1792528133528-bc61b018`, ref `research-e2e-live`, suite
  `research-agents-e2e`, origin `live`, workers `1`.
- Result: **3/3 trials passed** (research 2/2, guardrail 1/1), pass rate 1.0.
- Topic: `crispr-offtarget` (`biomed` invariants), model `openai/gpt-4o-mini`.
- Retrieval per research trial: 29 sources found, 3 relevance-filtered, 3 real
  scrapes succeeded from `arxiv.org`, 1 search call, 3 bodies, 3 citations.
- Artifact per trial: 5,492 / 5,665 chars; SHA-256
  `893bc071...42e97` / `10310c14...f939`.
- Wall: 36.1s and 34.0s (research), 13.8s (guardrail, no network).
- Proxy: 8 calls, 8,695 input / 2,606 output tokens; estimated **$0.00287** at
  $0.15/$0.60 per 1M; p50 2,253ms, p95 12,084ms; 8/8 ok.
- Trace: `15a63fb981d243333d9f9716d0f52af5`, 64 spans: 3
  `eval.research_e2e`, 8 `llm.call`, 2 `eval.retrieval.search`, 6
  `eval.retrieval.scrape`, 3 `eval.grade`, plus `node.*`, `search.federated`
  and `scrape.ScrapeRouter`. The supplied `traceparent` is the trace ID, so the
  target's spans join the caller's trace.
- Stored locally and synced to
  `s3://eval-store-342940030437-eu-north-1/eval-runs/runs/research-agents-e2e/research-e2e-live/run-1792528133528-bc61b018/`.
- Artifacts: `/var/lib/eval-runs/research-e2e-artifacts/15a63...-*.md`.
- Offline replay re-ran green (`run 1792528265349-492899b8`).

Trace UI (runner loopback; tunnel from your workstation):

```bash
ssh -i ~/.ssh/eval-runner-aws -L 16686:127.0.0.1:16686 ubuntu@13.60.60.51
# http://localhost:16686/trace/15a63fb981d243333d9f9716d0f52af5
```

## Good and bad examples

Good (real result):

```
PASS crispr-offtarget live (5492 chars, 29 sources, 3 bodies)
trace=15a63fb981d243333d9f9716d0f52af5
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

## Scheduling and operations

The live suite is cheap enough for a daily run, but it needs direct egress to
scholarly hosts, so it runs on the runner host (not the proxy-only Docker
sandbox). Agent A owns the shared units; the requested shape is:

```ini
# /etc/systemd/system/eval-research-live.service
[Unit]
Description=eval-service research real E2E (live)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=eval
Group=eval
WorkingDirectory=/srv/eval-service
Environment=EVAL_RESEARCH_PYTHON=/srv/eval-venvs/research-agents/bin/python
Environment=EVAL_RESEARCH_REPO=/srv/targets/research-orchestration-agents
Environment=EVAL_RESEARCH_MODEL=openai/gpt-4o-mini
Environment=EVAL_RESEARCH_TRACE_ENDPOINT=http://127.0.0.1:4318/v1/traces
Environment=EVAL_S3_URI=s3://eval-store-342940030437-eu-north-1/eval-runs
ExecStart=/srv/eval-service/integrations/research_agents/run_live_e2e.sh
TimeoutStartSec=1200
Nice=10
```

```ini
# /etc/systemd/system/eval-research-live.timer
[Unit]
Description=Daily research real E2E

[Timer]
OnCalendar=*-*-* 04:20:00 UTC
Persistent=true
RandomizedDelaySec=600

[Install]
WantedBy=timers.target
```

Manual trigger:

```bash
sudo systemctl start eval-research-live.service
journalctl -u eval-research-live.service -n 80 --no-pager
```

The service must not run in PR CI. The PR gate stays on `suite.yaml`
(`origin=replay`), and the live suite requires `origin=live` plus a fresh proxy
run. Keep the unit's `WorkingDirectory`/`EVAL_BIN` pointing at the canonical
checkout after this branch merges.

## Budget and safety rules

- Proxy: 80 requests / $0.25 lifetime per process; prices are required for the
  cost cap (fail closed). One process per scheduled run is fine; the proxy
  ledger is per process.
- Suite: per trial `max_total_tokens` 35,000, `max_model_calls` 32; topic
  `max_sources` 6, `max_scrapes` 8. A budget overflow fails the case.
- Overlap: `run_live_e2e.sh` holds an exclusive `flock`; the script also
  refuses to run under `CI`.
- Secrets: the key lives in SSM and the proxy process environment. The target
  gets `OPENAI_API_KEY=dummy-eval-proxy`; no key is written to the store,
  cassettes, spans, logs, or docs.
- Page contents are not emitted. Spans carry host names, char counts, token
  counts, and artifact hashes. The generated artifact is written outside git
  under the eval store for inspection.
