# PR gating, replay, and narrow-write notifications

This is the Phase 6 contract for turning stored runs into a merge gate
without letting pull requests read live credentials or change platform
policy.

## Hard rules

- The PR workflow uses `pull_request`, **never** `pull_request_target`.
  It gets `contents: read` only: no repository secrets, no AWS OIDC, no
  live provider keys.
- Fork PRs are allowed to run the offline gate, but they never receive
  secrets and never run a live suite. The gate is replay-only.
- Targets run through the Phase 4 Docker sandbox. The PR workflow uses an
  internal Docker network and the approved offline suite
  `examples/llm_calls/classify.yaml`; the fake provider needs no network.
- A live baseline is promoted only from a `push` or `workflow_dispatch`
  run on `refs/heads/main`, executed in the sandbox, with `origin=live`.
  `origin=replay` is never promotable.
- `watch.rs` remains read-only. Comments and statuses are a separate
  command with a separate, narrow-write token.

## Run provenance

`eval run` can attach provenance to a stored run:

```sh
eval run SUITE.yaml --ref work \
  --origin replay \
  --source-ref refs/pull/123/merge \
  --event pull_request \
  --sandbox docker --sandbox-network eval-pr
```

The sidecar is
`eval-runs/runs/<suite>/<ref>/run-<id>/provenance.json`:

```json
{
  "schema_version": 1,
  "origin": "replay",
  "source_ref": "refs/pull/123/merge",
  "event": "pull_request",
  "sandboxed": true
}
```

Missing provenance fails closed for promotion. Old runs therefore cannot
be silently promoted as live.

## Gate

`eval gate` compares a candidate to a baseline and writes the usual
`decision.json`:

```sh
eval gate --suite SUITE.yaml --store eval-runs \
  --baseline-ref baseline --candidate-ref work \
  --origin replay --json
```

`--origin replay` requires a candidate provenance sidecar and rejects a
live provenance mismatch. The exit code is `0` pass, `1` regression or
policy refusal, and `2` infrastructure/configuration error.

Promotion is explicit:

```sh
# The run must have origin=live, source_ref=refs/heads/main,
# event=push|workflow_dispatch, and sandboxed=true.
eval gate --suite SUITE.yaml --store eval-runs \
  --baseline-ref baseline --candidate-ref main-run --promote
```

The command refuses a failing candidate, a replay run, a non-main ref, a
PR event, or an unsandboxed run. This is the only supported live
promotion path; `eval run --set-baseline` is for local/dev scaffolding
and rejects `--origin replay`.

## PR workflow

`.github/workflows/pr-gate.yml`:

1. Builds the eval binary and the sandbox image.
2. Asserts that no live provider key is present.
3. Creates a trusted base checkout from `origin/<base>` and runs the
   approved offline suite there in the sandbox.
4. Runs the same suite against the PR checkout in the sandbox.
5. Calls `eval gate --origin replay`; a regression fails the check.

The workflow has no AWS or provider secrets, so a fork PR cannot read
them. The runner token is read-only. It does not post comments or
statuses; that is the separate notification lane below.

## Narrow-write notifications

Use a GitHub App or fine-grained token with only Issues/PR comment and
commit-status write permission. Keep the watcher's Issues-read token
separate:

```sh
export EVAL_GITHUB_WRITE_TOKEN=...   # write identity
export EVAL_WATCH_TOKEN=...          # read-only watcher identity
```

`eval notify` rejects `EVAL_WATCH_TOKEN` as its token source. Comments
carry a stable marker and are posted only if no existing comment
contains it:

```sh
eval notify \
  --repo owner/eval-service \
  --issue 42 \
  --body "eval gate: pass (run 179...)" \
  --marker "eval-gate:run-179..."
```

Statuses use a stable context, so repeated calls update one status rather
than creating parallel checks:

```sh
eval notify --repo owner/eval-service \
  --status-sha "$GITHUB_SHA" --status-state success \
  --status-context eval/gate
```

Run notifications from a trusted `workflow_run` or main-branch job, not
from a `pull_request` job that has a write token. The PR workflow's
GitHub check is already a merge signal; comments are advisory.

## What this does not do

- It does not run live LLM calls in PR jobs.
- It does not use `pull_request_target`.
- It does not let a PR promote a baseline or edit platform policy.
- It does not make the PR's eval engine a security boundary; the runner
  still treats PR code as untrusted and exposes no credentials to it.
