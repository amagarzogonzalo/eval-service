# Digest agent real E2E

This is the repeatable live lane for `digest_agent`. It uses the target's
real GitHub activity source, real model through `eval proxy`, real
ReportLab renderer, and a real SMTP protocol exchange into a local test
inbox. It never edits the target repository.

## Run

On the VM:

```bash
sudo systemctl start eval-live-digest.service
sudo journalctl -u eval-live-digest.service -f
```

Manual repository entry point:

```bash
make live-digest
```

The one-shot script is
`integrations/digest_agent/run_e2e.sh live`. It has an exclusive lock,
one worker, request/cost caps, and exits non-zero if the suite fails.

## Services

| Service | Implementation | Credential |
|---|---|---|
| Activity | target GitHub adapter, real REST | `/eval/github_watch_token` (read-only) |
| LLM | target live provider through `eval proxy` | `/eval/openrouter_api_key` |
| Renderer | target ReportLab adapter | none |
| Notifier | target SMTP adapter -> local test inbox | generated per run |
| Trace | OTLP/HTTP -> shared collector | none |

The SMTP sink is a real STARTTLS/AUTH SMTP exchange; messages and PDFs are
stored under `e2e-artifacts/<label>/mailbox/`. Set `SMTP_*` to use an
approved external staging SMTP host instead.

## Schedule

- systemd: `eval-live-digest.service` + daily `eval-live-digest.timer`
- cadence: 04:10 UTC with up to 5 minutes jitter
- budget: 120 proxy requests, **$0.20** cap
- trace endpoint: `http://127.0.0.1:4328`
- S3: `s3://eval-store-342940030437-eu-north-1/eval-runs`

## Observed evidence

Run `1792531414244-87b35627` (`origin=live`, `workers=1`):

- 3/3 cases pass, 4 trials
- real GitHub activity: 13 PRs, 47 commits, 142 files
- real model call through the proxy; bounded retry
- two generated PDFs and two authenticated SMTP messages with PDF
  attachments
- trace `00000000000031c1b3a5e58d1794c02b`
- artifacts under
  `/var/lib/eval-runs/e2e-artifacts/digest-e2e-live-20261020T212332Z`

## Failure behavior

- A bad GitHub repository case must fail closed without a model call.
- A model/provider failure is a graded failure, not a vacuous pass.
- The SMTP test sink and PDF are evidence; missing artifacts fail the
  structural graders.
- Replay is a separate offline suite and is never promoted as live.
