# E2E repeatability contract

A live digest or research run is only accepted when it can be executed
from this repository, repeatedly, without reconstructing ad-hoc shell
commands.

## Required artifacts per live integration

Use `<name>` = `digest` or `research`.

- `integrations/<project>/live_e2e.yaml` — committed live suite.
- Any adapter/config changes needed for the real service path.
- `datasets/<name>_e2e/` — real inputs, expected properties, and at least
  one deliberate positive and one negative/regression case.
- `docs/<name>-e2e.md` — services, credentials, egress, budget, cadence,
  manual trigger, recovery, and observed evidence.
- `Makefile` target — operator entry point:
  - `make live-digest`
  - `make live-research`
- `infra/aws/systemd/eval-live-<name>.service` — oneshot runner with a
  lock, budget, and trace export.
- `infra/aws/systemd/eval-live-<name>.timer` — automatic cadence.
- OTel trace correlation (`trace.json` / trace IDs) and S3 sync.

## Invocation rules

Automatic:

```bash
systemctl list-timers 'eval-live-*'
```

Manual, same code path as the timer:

```bash
sudo systemctl start eval-live-digest.service
sudo systemctl start eval-live-research.service
```

Developer entry point:

```bash
make live-digest
make live-research
```

The watcher remains offline-only unless a separate decision changes it.
Live runs are never part of PR CI.

## Safety

- Provider and service keys come from SSM/systemd credentials, never the
  repository.
- Live runs use `--workers 1`, a request/cost budget, and an overlap lock.
- External side effects use test accounts/destinations.
- Replay remains the deterministic CI gate; live runs produce evidence,
  not PR gate results.
- If a required real-service credential is unavailable, the agent must
  report the blocker; it must not silently substitute a fake.

## Acceptance checklist

- [ ] Committed live suite.
- [ ] Real inputs and positive/negative cases.
- [ ] Reproducible Make target.
- [ ] systemd service and timer.
- [ ] Manual trigger verified on the VM.
- [ ] Automatic timer verified on the VM.
- [ ] Budget/lock behavior verified.
- [ ] S3 artifacts and OTel trace verified.
- [ ] Docs include observed run ID and trace ID.
- [ ] No secrets or generated artifacts committed.
