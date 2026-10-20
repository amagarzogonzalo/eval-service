# Docker sandbox (Phase 4)

`eval run --sandbox docker` executes a **oneshot** target in a fresh
container. The target can reach exactly one thing: the eval proxy on an
internal Docker network. It has no internet, no external DNS, no cloud
metadata, no Docker socket, and no host mounts except the read-only
paths the operator lists.

## Topology

```text
 host                                   eval-sandbox (internal)     eval-sandbox-egress
 ─────                                  ──────────────────────      ─────────────────────
 eval run --sandbox docker  ──spawn──▶  target container  ──HTTP──▶  eig-proxy (172.30.0.2:8790)
                                                                          │
                                                                          └──HTTPS──▶ provider
```

- The **target** is attached only to the `--internal` network.
- The **proxy** is the only container with an egress network. It holds
  the real provider key; the target gets a dummy key and the proxy IP.
- `--dns 127.0.0.1` is applied to the target: external names cannot be
  resolved. The proxy IP is passed explicitly, not by DNS.
- The host proxy is not used directly from the target; a sidecar keeps
  the key out of the target namespace and survives target restarts.

## Build and verify

```sh
make sandbox-image     # ubuntu:24.04 + python3/python3-venv
make sandbox-verify    # command-shape tests + Docker isolation proof
```

The second target runs `EVAL_DOCKER=1` adversarial checks:

- internal proxy reachable (HTTP 200);
- `1.1.1.1:80` and `169.254.169.254:80` not reachable;
- external DNS does not resolve;
- `/var/run/docker.sock` absent;
- the provider key is absent from the target env and `/proc/1/environ`.

The normal workspace suite stays Docker-free; this test skips unless
`EVAL_DOCKER=1`.

## Run a target

```sh
# Build the target/venv mounts first; they are read-only.
docker network create --internal --subnet 172.30.0.0/24 eval-sandbox
docker network create eval-sandbox-egress

docker run -d --name eval-proxy-sbx \
  --network eval-sandbox --ip 172.30.0.2 \
  --network eval-sandbox-egress \
  --user 1000:1000 \
  -v /srv/eval-service/target/release/eval:/eval:ro \
  -v /var/lib/eval-runs:/var/lib/eval-runs \
  -e OPENROUTER_API_KEY \
  ubuntu:24.04 /eval proxy --bind 0.0.0.0 --port 8790 \
    --key-env OPENROUTER_API_KEY --provider openrouter \
    --project digest-agent --tag aws-sandbox \
    --store /var/lib/eval-runs \
    --max-requests 200 --max-cost-usd 0.25 \
    --price-in-per-1m 0.15 --price-out-per-1m 0.60 \
    --upstream-timeout-s 180 \
    --cassette-dir /var/lib/eval-runs/cassettes/aws-sandbox \
    https://openrouter.ai/api/v1

EVAL_DIGEST_PYTHON=/srv/eval-venvs/digest-agent/bin/python \
EVAL_DIGEST_SRC=/srv/targets/digest_agent/src \
EVAL_DIGEST_LIVE=1 \
EVAL_DIGEST_API_KEY=dummy-eval-proxy-observes \
EVAL_DIGEST_ENDPOINT=http://172.30.0.2:8790/v1 \
EVAL_DIGEST_MODEL=openai/gpt-4o-mini \
EVAL_DIGEST_TIMEOUT_S=90 EVAL_DIGEST_MAX_REQUESTS=8 \
  ./target/release/eval run integrations/digest_agent/live_bench.yaml \
    --sandbox docker \
    --sandbox-network eval-sandbox \
    --sandbox-image eval-sandbox:24.04 \
    --sandbox-mount /srv/eval-venvs/digest-agent \
    --sandbox-mount /srv/targets/digest_agent \
    --sandbox-user 65534:65534 \
    --workers 1 --ref aws-sandbox --store /var/lib/eval-runs
```

The target container gets only `EVAL_DIGEST_*` from the suite allowlist,
plus a sandbox-owned `PATH`, `HOME=/tmp`, `TMPDIR=/tmp`, and
`PYTHONDONTWRITEBYTECODE=1`. Host writes happen only through the proxy
store mount and the runner's store; the target filesystem is read-only
with a tmpfs `/tmp`.

## Isolation flags

Every invocation gets `--read-only`, `--tmpfs /tmp`, `--cap-drop ALL`,
`--security-opt no-new-privileges`, `--pids-limit`, `--memory`,
`--cpus`, `--user`, `--network <internal>`, and `--dns 127.0.0.1`.
Mounts are always `:ro`. There is no `--privileged`, no Docker socket,
and no host network.

## Limits

- **Oneshot targets only.** Worker-mode targets keep one long-lived
  process; sandbox support there is a later change.
- **Linux runners only.** Native Windows (`docker.exe` path semantics)
  is rejected; use WSL2 or the Linux runner.
- The adapter interpreter is the image's `python3`; the target's own
  venv/source must be passed with `--sandbox-mount` and read by UID
  `65534` (or the chosen `--sandbox-user`).
- The proxy sidecar is operator-managed. `eval watch` will own its
  lifecycle in Phase 5; for now, start/stop it explicitly.
- A target that needs writable scratch outside `/tmp` will need an
  explicit writable mount; none is provided by default.
