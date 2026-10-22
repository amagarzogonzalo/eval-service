# Production runbook

This is the operator path for the private, single-user deployment. It is not a
multi-tenant public-service runbook.

## Topology

| Component | Location |
|---|---|
| Checkout | `/srv/eval-service` |
| Canonical run store | `/var/lib/eval-runs` |
| S3 mirror | `$EVAL_STORE_S3` |
| Private service | `eval-private.service`, loopback `:8090` |
| Private access | SSH tunnel to the VM |
| Production scheduler | GitHub Actions self-hosted runner |
| Digest/research jobs | `eval-live-digest.service`, `eval-live-research.service` |
| Service environment | `/etc/eval/private-site.env`, `/etc/eval/research*.env` |

Never put secrets in a ticket, chat, or commit. The token and signing key live
only in the service environment.

## Access the UI

```sh
ssh -N \
  -o ExitOnForwardFailure=yes \
  -i "$EVAL_VM_SSH_KEY" \
  -L 19090:127.0.0.1:8090 \
  "$EVAL_VM_USER@$EVAL_VM_HOST"
```

Open `http://localhost:19090/` and enter the site API token. The token is
stored in `/etc/eval/private-site.env` on the VM and is never printed by the
application.

If the token is lost, rotate it:

```sh
sudo sh -c 'umask 077; printf "EVAL_SITE_API_TOKEN=%s\n" "$(openssl rand -hex 32)" \
  > /etc/eval/private-site.env; \
  printf "EVAL_SITE_SIGNING_KEY=%s\n" "$(openssl rand -hex 32)" \
  >> /etc/eval/private-site.env'
sudo systemctl restart eval-private
```

Restarting invalidates existing sessions.

## Scheduled production run

The workflow is `.github/workflows/live-production.yml`:

- schedule: Sunday 02:00 UTC
- manual: `workflow_dispatch`
- runner: self-hosted ARM64 labels
- jobs: digest and research, each with a judge step

A manual production run:

```sh
sudo systemctl start eval-live-research.service
sudo systemctl show -p Result --value eval-live-research.service
sudo bash -c 'set -a; source /etc/eval/research.env; source /etc/eval/research-service.env; set +a; \
  python3 /srv/eval-service/web/judge_run.py \
  --suite research-agents-service --ref research-service-remote'
```

The research suite uses the RAG topic in
`integrations/research_agents/live_service.yaml`.

## Deploy

```sh
cd /srv/eval-service
sudo -u eval git fetch origin
sudo -u eval git reset --hard origin/main
sudo cp web/private_ui/index.html web/private_ui/app.js web/private_ui/styles.css /srv/private-ui/
sudo chown eval:eval /srv/private-ui/index.html /srv/private-ui/app.js /srv/private-ui/styles.css
sudo systemctl restart eval-private
systemctl is-active eval-private
curl -fsS http://127.0.0.1:8090/healthz
```

The static UI is copied separately from the Python service. Updating the
checkout alone does not update `/srv/private-ui`.

## Verify a run

1. Open the latest research run.
2. Check query, sources, answer, and deterministic checks.
3. Open Judge and read the rationale and issues.
4. Compare with the previous run in Benchmarks, using the same query/model
   filter.
5. Check the GitHub Actions run result.

A judge score is advisory. A failed target run is a real result and must not be
excluded from the pass-rate calculation.

## Failure handling

| Symptom | Action |
|---|---|
| Service inactive | `systemctl status eval-private`, `journalctl -u eval-private -n 100` |
| UI 401 | Re-enter token; rotate if unknown |
| Run fails target checks | Inspect artifacts/sources; do not edit the target to pass |
| Judge fails closed | Check free model/key/quota; judge is advisory |
| Workflow did not run | Check self-hosted runner status and VM power |
| Disk pressure | Run `scripts/retention.py` in dry-run mode first |
| Lost local store | Restore from S3 using the restore drill |

## Restore drill

The restore is tested against a temporary directory, not over the live store:

```sh
STORES3="$EVAL_STORE_S3" make restore-drill
```

This syncs the S3 mirror into a temporary directory and validates metadata,
manifests, and run directories with `scripts/verify_store.py`. Never overwrite
`/var/lib/eval-runs` during a drill.

## Retention

The retention helper is dry-run by default:

```sh
python3 scripts/retention.py --store /var/lib/eval-runs --keep-latest 20 --days 365
```

Add `--apply` only after reviewing the plan. Preserve the latest run per
suite/ref and never delete a run that is linked from a report or a release.

## S3 bucket controls

The runner role may be object-only. Bucket-level configuration requires
separate permission and must be applied by an operator:

```sh
BUCKET="$EVAL_STORE_BUCKET" scripts/configure_store_bucket.sh
```

This enables versioning and applies `deploy/private/s3-lifecycle.json`.
The lifecycle keeps 730 days of current evidence, expires noncurrent versions
after 30 days, and aborts incomplete multipart uploads.

## Cost

The VM is not covered by AWS free tier. Check Cost Explorer after 24–48 hours;
billing is delayed. Set budget alerts before changing instance size. Stopping
the VM saves compute cost but also stops the self-hosted scheduled runner.

## Rollback

```sh
cd /srv/eval-service
sudo -u eval git reset --hard <previous-commit>
sudo cp web/private_ui/index.html web/private_ui/app.js web/private_ui/styles.css /srv/private-ui/
sudo systemctl restart eval-private
```

The run store is data, not code. A code rollback does not delete evidence.

## Post-incident checklist

- Record the run IDs and timestamps.
- Preserve logs and manifests.
- Rotate any exposed token or key immediately.
- Verify S3 mirror and restore path.
- Add a regression test or runbook note.
- If the target was at fault, record that; do not patch the target to hide it.

## Heartbeat and retention timer

Set a repository secret `HEARTBEAT_URL` (healthchecks.io or equivalent). Both
production jobs ping it after a successful run. The workflow skips the ping
when the secret is absent; a skipped heartbeat is not evidence of success.

Install the retention timer only after reviewing the dry-run:

```sh
sudo cp deploy/private/eval-retention.service deploy/private/eval-retention.timer \
  /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now eval-retention.timer
systemctl list-timers eval-retention.timer
```

The timer keeps the newest 20 runs per suite/ref and deletes only runs older
than 365 days. The script is dry-run by default; the unit explicitly uses
`--apply`.
