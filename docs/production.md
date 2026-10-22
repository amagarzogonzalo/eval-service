# Production service runbook

This is the operator entry point for the private `eval-service`
deployment. It covers what is deployed, how a weekly run happens, how to
deploy and verify, and how to recover. Deeper detail lives in the
focused documents linked below.

## Start here

| I want to... | Do this | Read |
|---|---|---|
| Work on the engine | `make build`, `make quality` | [`quality.md`](quality.md) |
| Validate an integration | `make check SUITE=path/to/suite.yaml` | [`integration.md`](integration.md) |
| Trigger a production run | `gh workflow run live-production.yml` | this file, *Weekly schedule* |
| Inspect evidence | SSH tunnel, then open the private UI | [`private-access.md`](private-access.md) |
| Interpret a chart or score | use the current/legacy rules | [`benchmark-evidence.md`](benchmark-evidence.md) |
| Verify a RAG answer | follow the source-by-source walkthrough | [`rag-verification.md`](rag-verification.md) |
| Fix a failed service/run | follow the triage table | [`troubleshooting.md`](troubleshooting.md) |
| Handle an old run | do not compare it to current data | [`legacy-runs.md`](legacy-runs.md) |

## What is deployed

```text
workstation ──SSH tunnel:19090──► VM 127.0.0.1:8080 (Caddy, Basic Auth)
                                      │
                                      ▼
                          eval-private.service :8090
                          (read-only API + static UI)
                                      │
                                      ▼
                          /var/lib/eval-runs  ──append-only sync──► S3
                                      ▲
GitHub Actions (weekly, ARM64) ──► systemd oneshot services
  digest: eval-live-digest.service
  research: eval-live-research.service
                                      │
                                      ▼
                          eval (Rust) ──► integration adapters
                                      │
                                      ▼
                          Phoenix / OTel Collector :4328/:6006
```

- **Rust engine**: `eval` is a Rust workspace (`crates/eval-*`) that
  executes adapters over the versioned `eval/v1` subprocess protocol,
  grades results, stores immutable evidence, compares runs, and reports.
- **Targets**: the production lanes are `integrations/digest_agent/`
  and `integrations/research_agents/`. Target repositories live under
  `/srv/targets/` and are never edited by this repository.
- **AWS**: Terraform in `infra/aws/` provisions a private-by-default
  VPC, an EC2 `t4g.small` ARM64 runner, an encrypted/versioned S3
  evidence bucket, IAM instance profile, SNS alarms, and a budget. See
  [`../infra/aws/README.md`](../infra/aws/README.md) for the exact
  resources and costs.
- **Scheduling**: `.github/workflows/live-production.yml` runs weekly
  on a self-hosted ARM64 runner and starts the systemd oneshot units.
  `workflow_dispatch` is the manual trigger.
- **Private UI**: `eval-private.service` serves read-only evidence on
  `127.0.0.1:8090`; Caddy enforces Basic Auth on `127.0.0.1:8080`.
  See [`private-access.md`](private-access.md).
- **Observability**: Phoenix + OpenTelemetry Collector provide trace
  inspection; runs remain canonical in the store/S3. See
  [`observability.md`](observability.md).

## Weekly schedule and manual runs

Canonical schedule: **Sunday 02:00 UTC**, plus manual dispatch. The
workflow uses the `live-production` concurrency group with
`cancel-in-progress: false`, so an overlapping run waits rather than
cancelling evidence.

```sh
gh workflow run live-production.yml
gh run watch
```

The workflow has two independent jobs, both on
`[self-hosted, linux, ARM64, eval-runner]`:

| Job | Starts | Then |
|---|---|---|
| `digest` | `eval-live-digest.service` | prints systemd `Result`; runs the free content judge for `digest-agent-e2e`/`live` |
| `research` | `eval-live-research.service` | prints systemd `Result`; runs the free content judge for `research-agents-service`/`research-service-remote` |

Manual, same code path:

```sh
sudo systemctl start eval-live-digest.service
sudo systemctl start eval-live-research.service
sudo systemctl show -p Result --value eval-live-digest.service
sudo journalctl -u eval-live-digest.service -n 100 --no-pager
```

The systemd timers under `infra/aws/systemd/` are optional fallbacks.
Current practice is workflow-only; keep the timers disabled. **Do not
enable a timer and the weekly workflow at the same time** — they would
produce duplicate runs.

Honest workflow limitation: the judge step ends with `|| true`, so a
judge failure does not fail the GitHub job. Verify `judge.json` on the
run; a missing judge is `n/a`, not a pass.

## Production lanes and evidence

| Lane | Suite / ref | What is real | Model record |
|---|---|---|---|
| Digest | `digest-agent-e2e` / `live` | real GitHub, entrypoint, PDF, SMTP test sink | target free Gemini provider |
| Research (remote) | `research-agents-service` / `research-service-remote` | deployed Render service, its retrieval, artifact, sources | `target-service:remote` because the target does not expose the upstream LLM |
| Research (local) | `research-agents-service` / `research-service-local` | local target service + free model through eval-proxy | the pinned free model |
| Replay/CI | target cassette/replay suites | deterministic fixtures | `origin=replay`, never live evidence |

Current run manifests carry lane, model, target version, free-only,
metrics, evidence, and provenance. A run without a manifest/model is
**legacy** and is excluded from default benchmark charts. See
[`legacy-runs.md`](legacy-runs.md) and
[`benchmark-evidence.md`](benchmark-evidence.md).

Current production research is intended to use the RAG/LLM topic
`How retrieval-augmented generation reduces hallucinations in large
language models` (`datasets/research_e2e/topics.json`,
`rag-hallucinations`). The research service suite still hardcodes
`crispr-offtarget`; that is a known implementation gap documented in
*Known gaps* below.

## Deploy an update

Run as an operator with sudo; do not commit generated artifacts.

```sh
# 1. Back up the static UI before replacing it
sudo cp -a /srv/private-ui /srv/private-ui.bak-$(date -u +%Y%m%dT%H%M%SZ)

# 2. Fetch the merged main commit and build the release binary
#    The deployment checkout must be clean (no local edits).
sudo -u eval git -C /srv/eval-service status --short
sudo -u eval git -C /srv/eval-service fetch --prune origin
sudo -u eval git -C /srv/eval-service checkout main
sudo -u eval git -C /srv/eval-service pull --ff-only
sudo -u eval bash -lc 'cd /srv/eval-service && make release'

# 3. Install/refresh units and static UI
sudo cp /srv/eval-service/deploy/private/eval-private.service \
  /srv/eval-service/deploy/private/eval-caddy.service /etc/systemd/system/
sudo install -d -m 755 /srv/private-ui
sudo cp -R /srv/eval-service/web/private_ui/. /srv/private-ui/
sudo systemctl daemon-reload
sudo systemctl restart eval-private.service eval-caddy.service
```

Expected: `systemctl is-active eval-private.service eval-caddy.service`
prints `active` twice; `curl -sS http://127.0.0.1:8090/healthz` prints
`{"ok": true}`. The private UI is reached through the tunnel; see
[`private-access.md`](private-access.md).

Do not copy secrets into the repository or shell history. The private
environment file remains `/etc/eval/private-site.env` (root-only).

## Verify after a run

```sh
# Service result and recent logs
sudo systemctl show -p Result --value eval-live-digest.service
sudo journalctl -u eval-live-digest.service -n 80 --no-pager

# Latest stored runs (canonical)
ls -lt /var/lib/eval-runs/runs/digest-agent-e2e/live | head
ls -lt /var/lib/eval-runs/runs/research-agents-service/research-service-remote | head

# Manifest / evidence for a run
RUN=/var/lib/eval-runs/runs/digest-agent-e2e/live/run-<run-id>
jq '{lane,model,free_only,evidence,provenance}' "$RUN/manifest.json"
jq '.cases[] | {case_id,pass}' "$RUN/metadata.json"

# S3 mirror (append-only; no --delete)
aws s3 ls s3://eval-store-342940030437-eu-north-1/eval-runs/runs/ | tail
```

Open the latest run in the private UI and confirm: query, sources,
citations, judge dimensions, and trace ID. For research, use
[`rag-verification.md`](rag-verification.md) before calling the answer
verified.

## Storage, retention, and immutability

- Canonical store: `/var/lib/eval-runs`.
- S3 mirror: `s3://eval-store-342940030437-eu-north-1/eval-runs`
  (versioned, encrypted, private; sync never uses `--delete`).
- Run directories are write-once; a different byte sequence for an
  existing run id is rejected. Baselines are pointers, not copies.
- `manifest.json` is required for a current run; absent/invalid
  manifests are legacy or `manifest_error`, never silently upgraded.
- Missing metrics are `n/a`, never `0`. See
  [`run-metrics.md`](run-metrics.md) for the full manifest and
  retention policy.
- No automated retention job is enabled. Recommended tiers: baselines and
  decision records forever; newest N (>=20) runs per suite/ref forever;
  other local run directories 180 days after an S3 archive; S3 object
  versions 365 days; abandoned multipart uploads 7 days. Do not delete
  local evidence before verifying the S3 copy; restore from a version
  instead of rewriting. Full policy: [`run-metrics.md`](run-metrics.md).

## Rollback

The data path is immutable; rollback is about code and presentation.

```sh
# Pick the previous known-good commit (recorded in the PR/run manifest)
sudo -u eval git -C /srv/eval-service checkout <known-good-sha>
sudo -u eval bash -lc 'cd /srv/eval-service && make release'
sudo systemctl restart eval-private.service eval-caddy.service

# Roll back the static UI from the backup taken before the deploy
sudo rm -rf /srv/private-ui && \
  sudo cp -a /srv/private-ui.bak-<stamp> /srv/private-ui
```

For a unit regression, restore the previous service file from the
known-good commit, then `daemon-reload` and restart. Do not delete run
directories or S3 objects to "roll back" a benchmark; keep the evidence
and fix the code.

## Incident recovery

Use [`troubleshooting.md`](troubleshooting.md) for the full triage
table. The shortest path:

1. `systemctl status` / `journalctl -u` for the failed unit.
2. `curl -sS http://127.0.0.1:8090/healthz` for the private service.
3. Check the latest run `metadata.json`/`manifest.json` and `grades.json`.
4. Re-run the workflow manually only after the root cause is addressed;
   never delete a failed run to make the dashboard green.

## Known gaps and limitations

- Research remote lane records the upstream model as
  `target-service:remote`; internal tokens, cost, and spans are `n/a`.
- The research service suite still hardcodes `crispr-offtarget`; the RAG
  topic is present in the dataset and is the adapter default, but the
  production suite must be updated before claiming RAG production runs.
- Research can be flaky because retrieval/scrape is external. A failed
  run is evidence, not something to hide.
- Remote research has previously returned a wiki/source wrapper with
  zero sources and empty citation markers; the suite fails closed. See
  [`research-e2e.md`](research-e2e.md).
- The content judge is advisory: no human-labelled calibration set
  exists. It is not a gate. See [`judge-calibration.md`](judge-calibration.md).
- Single `t4g.small`, no HA/failover. The free trial ends 2026-12-31.
- Terraform apply is manual/local; there is no apply role/workflow.
- No automated retention job; S3 versioning is the recovery mechanism.
- The private UI is read-only, single-operator, and has no multi-tenant
  RBAC.

## Engineering highlights (verifiable, not marketing)

- **Rust platform**: versioned protocol, conformance and Rust/Python
  parity tests, immutable run store, graders/statistics, proxy and
  cassette machinery, CLI. See `crates/`, `docs/02-architecture.md`,
  `docs/03-protocol-v1.md`.
- **Terraform + AWS**: VPC/EC2 ARM64 runner, encrypted versioned S3,
  IAM instance role, SNS alarms, budget stop action, OIDC plan/drift
  lanes. Apply remains intentionally manual.
- **Operations**: weekly GitHub Actions on a self-hosted ARM64 runner,
  systemd oneshot units, private Caddy + token/session auth, Phoenix/OTel
  traces, S3 evidence mirror.
- **Honest measurement**: free-only policy, `n/a` for absent metrics,
  current/legacy separation, deterministic gates, advisory judges.

These are the parts worth showing in a portfolio because they are
implemented, tested, and observable in this repository.
