# `eval watch` — read-only GitHub polling (Phase 5)

`eval watch` turns labelled GitHub issues into evaluation runs without
letting a PR or an issue change platform policy. This phase is
**read-only on GitHub**: it fetches issues, never comments, labels,
closes, or changes statuses. The narrow-write token and PR comments are
Phase 6 work.

## Request contract

An open issue with the configured label (default `eval-request`) needs
one fenced block:

````markdown
```eval-request
{"suite": "integrations/digest_agent/suite.yaml"}
```
````

Rules:

- `suite` is required and must match an entry in `watch.suites` exactly.
  The issue cannot select an arbitrary file, target, ref, or environment.
- The issue body may contain other text; the first `eval-request`
  block is parsed.
- Unknown fields inside the block are rejected.
- The run uses the configured `ref_name`, `store`, `workers`, and
  `root`; nothing in the issue can override them.
- Pull requests are ignored (only issues are polled).

The issue source does not need a write scope. A fine-grained token with
read-only **Issues** access (and repository metadata) is enough.

## Configuration

Paths are resolved relative to the config file, which makes systemd and
cron launches independent of the current directory.

```toml
repo = "owner/eval-service"
base_url = "https://api.github.com"
label = "eval-request"
token_env = "EVAL_WATCH_TOKEN"
poll_interval_seconds = 60
max_pages = 5

state_path = "eval-watch/state.json"
# heartbeat_path = "eval-watch/heartbeat.json"
debounce_seconds = 30
stale_run_seconds = 7200

store = "eval-runs"
ref_name = "watch"
workers = 1
max_attempts = 2
root = "."

suites = [
  "integrations/digest_agent/suite.yaml",
  "datasets/cv_demo/suite.yaml",
]

# Optional: refresh the portfolio dashboard after every completed run.
# workspace = "eval-workspace.toml"
# dashboard_out = "dashboard"

# Required: no unattended host execution.
[sandbox]
kind = "docker"
image = "eval-sandbox:24.04"
network = "eval-sandbox"
mounts = [
  "/srv/targets/digest_agent",
  "/srv/eval-venvs/digest-agent",
]
memory_mb = 2048
cpus = 2.0
pids = 128
user = "65534:65534"
```

| Key | Meaning |
|---|---|
| `repo` | `owner/name` polled with the GitHub REST API. |
| `label` | Issue label that marks an eval request. |
| `token_env` | Environment variable holding the read-only token. Never inline the token. |
| `poll_interval_seconds` | Delay between polls. |
| `max_pages` | Pagination cap (`100` issues per page). |
| `state_path` | Atomic JSON queue/state file. |
| `heartbeat_path` | Optional; defaults to `heartbeat.json` beside the state file. |
| `debounce_seconds` | Wait this long after the first sighting before running; edits create a new request id. |
| `stale_run_seconds` | Threshold for a `running` entry in the heartbeat to count as stale. |
| `store` | Run store root. |
| `ref_name` | Ref namespace for watch runs (`watch`). |
| `workers` | Worker slots passed to the runner. |
| `max_attempts` | Attempts before a failed request is parked. |
| `root` | Files root for suite targets. |
| `suites` | Hard allowlist of suite paths. |
| `[sandbox]` | **Required.** Docker image, internal network, read-only mounts, limits, and non-root user. |
| `workspace`, `dashboard_out` | Optional dashboard refresh. |

### Mandatory Docker sandbox

`eval watch` refuses to start unless `[sandbox]` is present, validates,
and uses a dedicated internal network. There is no `--no-sandbox` or
host-execution fallback: an Issue can trigger work but can never cause
unattended host execution.

The watcher passes the sandbox policy through the existing Phase 4
`RunPolicy.sandbox`; it does not implement or alter the Docker executor.
The internal network and proxy sidecar are operator-managed:

```sh
docker network create --internal --subnet 172.30.0.0/24 eval-sandbox
docker network create eval-sandbox-egress
# start the eval proxy sidecar per docs/sandbox.md
```

Mount target checkouts and target venvs read-only. The suite's files
root is mounted read-only automatically; the container has a tmpfs
`/tmp`, no Docker socket, no host network, no external DNS, and no
cloud metadata route.

## Queue, idempotency, and restart safety

The state file records every request and its status:

- `pending`: seen, waiting for the debounce or a retry window.
- `running`: claimed by a watcher before execution.
- `done`: has a `run_id` and pass/fail result.
- `failed`: attempts exhausted; parked for an operator.

The request id is a blake3 hash of the repository, issue number, body
hash, and suite. Consequences:

- Polling the same unchanged issue again is a no-op.
- Editing the issue body changes the id and creates one new request.
- A comment edit alone does not rerun the request unless the body
  changes.

Every run stores the request id in `metadata.json` (`request_id`).
Before executing a claimed request — and again on restart — the watcher
looks for a stored run with that id. That closes the normal crash
window between "run finished" and "state saved": recovery adopts the
stored run instead of running it again. Interrupted requests without a
stored run are returned to `pending`; failed runs retry with
exponential backoff up to `max_attempts`.

A lock file (`.lock` beside the state file) prevents two watchers from
sharing a state directory. If a process crashed, remove the lock only
after confirming no watcher is alive.

## Heartbeat and stale-run alarm

`heartbeat.json` is written after every poll and every run:

```json
{
  "schema_version": 1,
  "repo": "owner/eval-service",
  "pid": 1234,
  "last_beat_ms": 1792483529000,
  "last_poll_ms": 1792483528000,
  "last_run_ms": 1792483528505,
  "last_run_id": "1792483528505-d693cedb",
  "pending": 0,
  "running": [],
  "failed": 0,
  "stale_running": 0,
  "last_error": null
}
```

A monitor should alert when any of these holds:

- `last_beat_ms` is older than the expected poll interval plus slack.
- `stale_running > 0` (a run has exceeded `stale_run_seconds`).
- `last_error` remains non-null across consecutive polls.
- `pending` grows without `last_run_ms` advancing.

The watcher writes the heartbeat before a long run, so a hung target is
visible as a stale `last_beat_ms`/`running` entry rather than silence.
On the runner, point `heartbeat_path` at
`/var/lib/eval-runs/watch-heartbeat.json` so `eval-store-sync` copies it
to S3 with the rest of the evidence.

## Running

```sh
export EVAL_WATCH_TOKEN=...        # read-only Issues token
eval watch --config eval-watch.toml                 # long-running
eval watch --config eval-watch.toml --once          # one poll/run cycle
eval watch --config eval-watch.toml --once --dry-run
```

Useful overrides: `--interval`, `--store`, `--state`, `--token-env`,
`--base-url`, `--debounce-seconds`.

Startup fails with exit 2 if `[sandbox]` is missing or unsafe (for
example `network = "host"`). That is intentional: there is no
unattended host-execution mode.

`--dry-run` fetches and prints due requests without saving or running
them. `--once` is the cron-friendly form; set `debounce_seconds = 0`
when a single poll should run immediately.

## systemd example

```ini
[Unit]
Description=eval watch (read-only GitHub polling)
After=network-online.target
Wants=network-online.target

[Service]
User=eval
WorkingDirectory=/srv/eval-service
EnvironmentFile=/etc/eval/watch.env
ExecStart=/srv/eval-service/target/release/eval watch --config /srv/eval-service/eval-watch.toml
Restart=on-failure
RestartSec=30

[Install]
WantedBy=multi-user.target
```

`/etc/eval/watch.env` holds `EVAL_WATCH_TOKEN=...` with mode `0600`.
The token is never written to the store, state, heartbeat, or logs.

## Security boundary

- Read-only: only `GET /repos/{owner}/{repo}/issues`.
- The issue body chooses only from the configured suite allowlist.
- Every run uses the Phase 4 Docker sandbox: internal network, read-only
  mounts, non-root user, limits, no Docker socket. Missing/unsafe
  sandbox config is a startup error.
- Targets run under the existing runner policy (env allowlist, cwd,
  deadlines) inside that container.
- No PR content is executed. PR gating is Phase 6.
- The state file and lock are local operational data; they contain no
  credentials, but they do contain issue numbers and run ids.

## What this does not do

- No PR comments, statuses, labels, or merge gating (Phase 6).
- No model or provider calls by the watcher itself; suites own their
  targets.
- No distributed queue: one watcher per state directory.
- No automatic promotion of a judge; calibration is separate.
