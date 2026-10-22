# Release and freeze checklist

Use this before tagging or recording the CV video. Placeholders must be filled
from the final run, not from memory.

## Inputs

- [ ] VM started and self-hosted runner online
- [ ] S3 versioning and lifecycle applied, or explicitly deferred
- [ ] Dead-man heartbeat configured
- [ ] Budget alerts configured
- [ ] Calibration labels supplied
- [ ] Model-variant decision recorded
- [ ] Agent B quality/security PR merged
- [ ] No open high-severity CI findings

## Final production evidence

- [ ] Manual production dispatch succeeds
- [ ] RAG research run passes
- [ ] Digest run passes
- [ ] Judge outputs recorded for both
- [ ] Restore drill succeeds from S3
- [ ] Retention dry-run produces no accidental deletions
- [ ] `make verify` passes
- [ ] `make python-test` passes
- [ ] Python lint passes
- [ ] Secret scan passes

## Freeze values

Fill these once and use them everywhere:

| Field | Value |
|---|---|
| Commit | `FINAL_COMMIT` |
| RAG run ID | `FINAL_RAG_RUN_ID` |
| Digest run ID | `FINAL_DIGEST_RUN_ID` |
| RAG query | `How retrieval-augmented generation reduces hallucinations in large language models` |
| RAG cases | `FINAL_CASES` |
| RAG sources | `FINAL_SOURCES` |
| RAG judge | `FINAL_JUDGE` |
| Digest judge | `FINAL_DIGEST_JUDGE` |
| p50 | `FINAL_P50` |
| Throughput | `FINAL_TPM` |
| Model labels | `FINAL_MODELS` |

## Deployment

- [ ] `/srv/eval-service` reset to the frozen commit
- [ ] Static UI copied to `/srv/private-ui`
- [ ] `eval-private` restarted and healthy
- [ ] Runner online
- [ ] Health endpoint responds on loopback
- [ ] SSH tunnel checked with no public exposure

## Public assets

- [ ] README numbers match the frozen run
- [ ] Case study numbers match the frozen run
- [ ] Screenshots have no tokens, account IDs, IPs, or emails
- [ ] Video brief uses only frozen numbers
- [ ] Release tag created
- [ ] Changelog updated

## Sign-off

- [ ] Owner approves metrics
- [ ] Owner approves security boundary
- [ ] Owner approves video asset pack
- [ ] Freeze declared: **no UI or benchmark changes during recording**
