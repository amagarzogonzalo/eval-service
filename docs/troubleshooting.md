# Troubleshooting and recovery

Use this when the private UI, a production run, or the weekly workflow
fails. Start with the smallest observable fact; do not “fix” a benchmark
by deleting its run.

## First five checks

```sh
# 1. Units
sudo systemctl is-active eval-private.service eval-caddy.service
sudo systemctl is-active eval-live-digest.service eval-live-research.service

# 2. Private app health (loopback)
curl -sS http://127.0.0.1:8090/healthz
# expected {"ok": true}

# 3. Recent logs
sudo journalctl -u eval-private.service -n 80 --no-pager
sudo journalctl -u eval-caddy.service -n 40 --no-pager

# 4. Latest run state
ls -lt /var/lib/eval-runs/runs/digest-agent-e2e/live | head
ls -lt /var/lib/eval-runs/runs/research-agents-service/research-service-remote | head

# 5. Runner resources
df -h /var/lib/eval-runs
free -h
```

## Triage table

| Symptom | Likely cause | Action |
|---|---|---|
| Tunnel connection refused | tunnel closed, VM down, Caddy down | reopen tunnel; `systemctl status eval-caddy`; check EC2 |
| Basic Auth repeats | wrong user/hash | verify `/etc/eval/private-site.env` as root; restart Caddy |
| `Invalid token` | wrong API token | re-read token with `sudo`; do not paste it into logs |
| UI loads, API 401 | 12 h session expired | log in again |
| UI blank / no runs | store path or permissions | check `ReadOnlyPaths`, `eval-private` logs, store ownership |
| Run 404 | wrong suite/ref/run id | open from Runs; do not guess path components |
| Legacy run hidden from charts | no `manifest.json`/model | expected; use “Include legacy runs” in Runs/Benchmark |
| `Result=exit-code` for a service | target/grader failure | open the run’s `grades.json`, then `journalctl` |
| Workflow failed before jobs | runner labels/offline | check self-hosted runner registration and labels |
| Workflow green, judge missing | judge step uses `|| true` | inspect `judge.json`; missing is `n/a` |
| Duplicate runs close together | timer and workflow both active | disable the timer; keep workflow as scheduler |
| `target-service:remote` | remote target hides model | expected; do not compare as a named model |
| Sources `0` / citations `0` | remote artifact wrapper or failed retrieval | fail closed; see `research-e2e.md` |
| S3 missing objects | sync failed or not run | run `aws s3 sync` without `--delete`; verify source first |
| Disk full | old artifacts/logs | archive to S3; never delete baselines/latest runs first |

## Private UI recovery

```sh
sudo systemctl restart eval-private.service
sudo systemctl restart eval-caddy.service
curl -sS http://127.0.0.1:8090/healthz
sudo journalctl -u eval-private.service -n 100 --no-pager
```

If static files are missing, redeploy the UI (see
[`production.md`](production.md), *Deploy an update*). Keep the
`/srv/private-ui.bak-*` copy until the login and a run detail page work.

If Caddy fails, the app on `8090` may still be healthy. Diagnose Caddy
separately; do not expose `8090` publicly to work around it.

## Production run recovery

```sh
sudo systemctl start eval-live-digest.service
sudo systemctl show -p Result -p ExecMainStatus eval-live-digest.service
sudo journalctl -u eval-live-digest.service -n 120 --no-pager

sudo systemctl start eval-live-research.service
sudo systemctl show -p Result -p ExecMainStatus eval-live-research.service
sudo journalctl -u eval-live-research.service -n 120 --no-pager
```

For a failed run, keep it. Inspect:

```sh
RUN=/var/lib/eval-runs/runs/digest-agent-e2e/live/run-<run-id>
jq '.cases[] | {case_id,pass,trials}' "$RUN/metadata.json"
jq '.cases[] | {case_id,grades}' "$RUN/grades.json"
cat "$RUN/manifest.json"
cat "$RUN/trace.json"
```

Then re-run the workflow only after the cause is fixed. A failed run is
evidence; it is not deleted to make the dashboard green.

## Workflow recovery

```sh
gh run list --workflow=live-production.yml --limit 10
gh run view <run-id> --log
gh workflow run live-production.yml
```

Check the self-hosted runner labels:
`[self-hosted, linux, ARM64, eval-runner]`. A missing/offline runner is
an infrastructure issue, not a benchmark result.

Workflow limitation: the judge command ends with `|| true`; a judge
failure does not turn the job red. Confirm `judge.json` separately.

## Authentication recovery

Follow [`private-access.md`](private-access.md) for rotation and session
expiry. Never print a token to debug it. `401` on `/api/*` is a session
problem; `401` from Caddy is Basic Auth; the two are different layers.

## Storage and S3 recovery

```sh
# Inspect the canonical store first
find /var/lib/eval-runs/runs -maxdepth 4 -name manifest.json | tail

# Verify the mirror (append-only; never --delete)
aws s3 ls s3://eval-store-342940030437-eu-north-1/eval-runs/runs/ | tail

# Reconcile after verifying the local store
aws s3 sync /var/lib/eval-runs \
  s3://eval-store-342940030437-eu-north-1/eval-runs --only-show-errors
```

If local evidence is present but S3 is missing, re-run the sync. If S3
has a version but local was damaged, restore that version rather than
rewriting the run. `metadata.json`, `observations.jsonl`, `grades.json`,
and `manifest.json` are write-once.

## Rollback

Code/UI rollback is in [`production.md`](production.md), *Rollback*.
Do not delete or rewrite run directories. If a code change caused bad
grades, keep the runs and fix/re-grade; the raw evidence is the source of
truth.

## Escalation evidence to collect

- timestamp and workflow/run ID,
- unit name and `Result`/exit code,
- last 80 log lines (redact tokens),
- run ID and trace ID,
- whether the run is current or legacy,
- the exact failing check/evidence line.

Do not attach secrets, cookies, full prompts, or page contents without
review.
