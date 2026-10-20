# AWS runner (Terraform + cloud-init)

Free-tier Phase 3 host: **EC2 `t4g.small`** (2 vCPU / 2 GiB, ARM64,
Ubuntu 24.04), 750 h/month free until **2026-12-31**. The instance is
private-by-default in practice: SSH is allowed only from `admin_cidr`;
everything else is egress. The eval store syncs to a versioned,
private S3 bucket (no `--delete`, append-only evidence).

## 0. Prerequisites (one-time, local)

AWS CLI and Terraform are not system-wide on this machine; the repo
uses local copies under `.tools/` (gitignored):

```powershell
py -m venv .tools\awscli
.tools\awscli\Scripts\python -m pip install awscli
# Terraform is downloaded from releases.hashicorp.com into .tools\terraform
```

Configure AWS credentials **yourself** (never paste secrets into
chat). Create a dedicated IAM user `eval-provisioner` with
`AdministratorAccess` for provisioning (delete the access key after
setup; enable MFA on the root account first):

```powershell
.\.tools\awscli\Scripts\python.exe -m awscli configure
# Access key, secret, region: eu-north-1, output: json
.\.tools\awscli\Scripts\python.exe -m awscli sts get-caller-identity
```

The account can be old (legacy Free Tier) or new (6-month Free plan).
The **T4g free trial applies to both** until 2026-12-31; check
Billing → Free Tier in the console for your current status.

## 1. SSH key

```powershell
ssh-keygen -t ed25519 -f $env:USERPROFILE\.ssh\eval-runner-aws -N '""'
```

## 2. Configure

```powershell
cd infra\aws
Copy-Item terraform.tfvars.example terraform.tfvars
# edit admin_cidr (your current public IP /32), alert_email, key path
```

Find your IP with `curl https://checkip.amazonaws.com`. The security
group accepts SSH only from that CIDR; update it if your ISP changes
your address.

## 3. Remote state, plan and apply

Remote state lives in a dedicated S3 bucket with native Terraform
lockfiles. On a fresh account, create it once:

```powershell
cd infra\aws\bootstrap
..\..\..\.tools\terraform\terraform.exe init
..\..\..\.tools\terraform\terraform.exe apply
cd ..\
```

The bucket name embeds the account id, so pass it to the main stack's
init (backends cannot use variables). Then:

```powershell
$bucket = ..\..\.tools\terraform\terraform.exe -chdir=bootstrap output -raw backend_bucket
..\..\.tools\terraform\terraform.exe init -backend-config="bucket=$bucket" -migrate-state -force-copy
..\..\.tools\terraform\terraform.exe plan -out tfplan
..\..\.tools\terraform\terraform.exe apply tfplan
```

This creates: VPC + public subnet + IGW, security group, IAM role +
instance profile (SSM, append-only S3 store, `/eval/*` parameters),
`t4g.small` instance, 30 GiB encrypted gp3 boot volume, versioned
private S3 store with TLS-only policy, SNS alarms (status checks, CPU
credits), and a **$10 monthly budget** with alerts and an automatic
**stop-instance action** at 100%.

Confirm the budget/SNS confirmation emails once.

Wait for bootstrap, then connect:

```powershell
ssh -i $env:USERPROFILE\.ssh\eval-runner-aws ubuntu@<public_ip>
cloud-init status --wait
uname -m          # aarch64
free -h           # 2 GiB RAM + 4 GiB swap
cargo --version
```

## 4. Load the platform

The eval-service repo is private. On the instance, create a read-only
GitHub deploy key and clone:

```bash
ssh-keygen -t ed25519 -f ~/.ssh/github -N ''
cat ~/.ssh/github.pub
# add it to the GitHub repo: Settings -> Deploy keys (read-only)
cat >> ~/.ssh/config <<'EOF'
Host github.com
  IdentityFile ~/.ssh/github
  IdentitiesOnly yes
EOF
chmod 600 ~/.ssh/config
sudo -u eval git clone git@github.com:amagarzogonzalo/eval-service.git /srv/eval-service
```

Build through the repo Makefile (`/etc/profile.d/eval.sh` sets
`CARGO_BUILD_JOBS=2`, `EVAL_PYTHON=python3`, and `EVAL_RUNNER=1`, so
make targets use the `/var/lib/eval-runs` store automatically):

```bash
sudo -u eval bash -lc 'cd /srv/eval-service && make release'
```

`make help` lists the runner targets (`build`, `verify`, `phase3-verify`,
`store-sync`, `store-restore`, ...). Raw cargo/eval commands still work;
make is the canonical entry point on the Linux runner.

Target venvs live **outside** the target checkout:

```bash
sudo -u eval python3 -m venv /srv/eval-venvs/digest-agent
sudo -u eval /srv/eval-venvs/digest-agent/bin/pip install -r <target>/requirements.txt
```

Secrets are read from SSM Parameter Store at job time (instance role
can read only `/eval/*`):

```bash
aws ssm put-parameter --name /eval/openrouter_api_key --type SecureString \
  --value '<key>' --region eu-north-1
aws ssm get-parameter --name /eval/openrouter_api_key --with-decryption \
  --query Parameter.Value --output text --region eu-north-1
```

## 5. Verify (Phase 3 exit)

```bash
cd /srv/eval-service
export EVAL_DIGEST_PYTHON=/srv/eval-venvs/digest-agent/bin/python
export EVAL_DIGEST_SRC=/srv/targets/digest_agent/src
make phase3-verify   # aarch64 + fmt + clippy + tests + offline digest probe
```

`make phase3-verify` covers the offline half of the checklist. Then run
the live cycle through `eval proxy` with `--workers 1`
(`make phase3-live` after the proxy is running and `EVAL_DIGEST_LIVE=1`
is set; it probes the live suite, then records baseline → work →
compare), and a cassette replay. `make store-sync
STORES3=s3://<bucket>/eval-runs` records the evidence copy. Keep the
suite out of CI.

## Phase 3 verification log

Verified on the AWS runner on 2026-10-20:

- `uname -m` = `aarch64`; `t4g.small` (2 vCPU) with 4 GiB swap.
- `make phase3-verify`: fmt, clippy, full workspace tests, and the
  offline `digest_agent` probe are green.
- **Digest live** through `eval proxy`: baseline → work → compare PASS,
  3/3 cases and 6/6 trials per run; 96/96 proxy calls OK; 35,891 input /
  21,556 output tokens; estimated **$0.0183** (gpt-4o-mini).
- **Digest cassette replay**: 3/3 cases, 6/6 trials, 48 `origin=replay`
  events, 0 ms replay latency.
- **Research replay** (target's own cassettes): baseline → work →
  compare PASS, ~11 s/run.
- **Research live** (real research workflow, live LLM, recorded
  search/scrape): 2/2 trials green; 20/20 proxy calls; 26,072 input /
  6,138 output tokens; estimated **$0.0076**; 32 s/trial.
- Store sync + restore round-trip: `diff -r` clean over 126 objects;
  S3 versioning is enabled and Terraform state lives in the S3 backend.
- Budget ($10), stop-at-100% budget action, status-check and CPU-credit
  alarms exist; alarms are `OK`. The SNS email subscription is still
  `PendingConfirmation`, so alarm/budget emails do not deliver yet.

Honest note: `openai/gpt-oss-20b:nitro` produced real upstream calls of
107 s and 114 s. The target's own timeout fired first and the suite
recorded those trials as failures; the green digest baseline uses
`openai/gpt-4o-mini` on the 2 GiB runner. The failed runs remain in the
store, not hidden.

Still open before the Phase 3 exit:

- Confirm the SNS subscription (and budget notifications), then
  test-fire an alarm.
- Terminate-and-rebuild drill.
- Fully live research retrieval (search/scrape currently replay from the
  target's cassettes; model calls are live).

## Phase 5 watch deployment (2026-10-20)

- PRs #1/#2 merged; runner pulled to the merge commit and the release
  binary rebuilt. `make verify` green on the runner.
- Internal networks `eval-sandbox` and `eval-sandbox-egress` created.
- `/etc/eval/eval-watch.toml` installed from
  `infra/aws/eval-watch.toml.example` (state
  `/var/lib/eval-watch/state.json`, heartbeat
  `/var/lib/eval-runs/watch-heartbeat.json`, store
  `/var/lib/eval-runs`, digest offline suite allowlisted, S3 post-run
  sync; sandbox network `eval-sandbox`).
- `infra/aws/systemd/eval-watch.service` and the alarm service/timer are
  installed. The alarm script was tested with fresh and stale heartbeats:
  fresh -> exit 0, stale -> exit 1 and flag written.
- `EVAL_DOCKER=1 cargo test -p eval-cli --test watch_tests` on the
  runner: 4/4 green, including the real sandbox run, debounce/restart,
  no-duplicate rerun, and the post-run hook (1.9 s).
- systemd fail-closed check: without `/etc/eval/watch.token` the unit
  exits at credential setup; with a dummy token it reaches the API and
  stops on HTTP 401. Neither path executes a target.

**Blocked on the read-only GitHub token.** No token exists under
`/eval/*`; the service is intentionally disabled. To finish:

```bash
aws ssm put-parameter --name /eval/github_watch_token \
  --type SecureString --value '<read-only Issues token>' --region eu-north-1

# on the runner, as root:
aws ssm get-parameter --name /eval/github_watch_token --with-decryption \
  --query Parameter.Value --output text --region eu-north-1 \
  | install -o root -g root -m 600 /dev/stdin /etc/eval/watch.token
systemctl enable --now eval-watch
systemctl enable --now eval-watch-alarm.timer
```

Until the token is installed, `eval-watch.service` must remain stopped:
it fails closed without a token and never executes on the host.

**Calibration is also blocked on data.** The repository only contains a
synthetic fixture. A real 30+ item / 10+ A/B-pair human-labelled set is
required before a non-synthetic report can be produced; labels are not
fabricated.

## Live cycle runbook (needs a provider key)

Store the key once (never echoed, never in the repo), then load it into
the proxy process only:

```bash
aws ssm put-parameter --name /eval/openrouter_api_key --type SecureString \
  --value '<key>' --region eu-north-1

cd /srv/eval-service
export OPENROUTER_API_KEY="$(aws ssm get-parameter \
  --name /eval/openrouter_api_key --with-decryption \
  --query Parameter.Value --output text --region eu-north-1)"

# Proxy observes and records; it is the only process with the real key.
./target/release/eval proxy --key-env OPENROUTER_API_KEY \
  --provider openrouter --project digest-agent --tag digest-live-proxy \
  --max-requests 200 --max-cost-usd 0.25 \
  --price-in-per-1m 0.15 --price-out-per-1m 0.60 \
  --upstream-timeout-s 180 \
  --store /var/lib/eval-runs \
  --cassette-dir /var/lib/eval-runs/cassettes/digest-live-p3 \
  https://openrouter.ai/api/v1 &

export EVAL_DIGEST_LIVE=1 EVAL_DIGEST_API_KEY=dummy-eval-proxy-observes \
  EVAL_DIGEST_ENDPOINT=http://127.0.0.1:8787/v1 \
  EVAL_DIGEST_MODEL=openai/gpt-4o-mini \
  EVAL_DIGEST_TIMEOUT_S=90 EVAL_DIGEST_MAX_REQUESTS=8 \
  EVAL_DIGEST_PYTHON=/srv/eval-venvs/digest-agent/bin/python \
  EVAL_DIGEST_SRC=/srv/targets/digest_agent/src

make phase3-live            # probe, baseline, work, compare (90 s apart)
kill %1

# Offline replay from the cassettes recorded above (starts/kills the
# cassette server itself; the endpoint env is overridden per run).
make phase3-replay CASSETTE_DIR=/var/lib/eval-runs/cassettes/digest-live-p3

make store-sync STORES3=s3://<bucket>/eval-runs
```

## Costs and guardrails (honest)

- **AWS Budgets are alerts and delayed actions, not hard caps.** The
  budget fires at 50%/80% actual and 100% forecast; the **budget
  action stops the instance at 100% actual**. AWS evaluates budgets
  periodically (not instantly), so this bounds — not eliminates —
  runaway spend.
- **Compute:** `t4g.small` hours are free until **2026-12-31** (750
  h/month); CPU credit mode is `standard` to avoid surplus-credit
  charges.
- **Fixed floor, even with free compute:** the public IPv4 is
  ~$3.65/month and 30 GiB gp3 EBS ~$2.40/month (≈$6/month if not
  covered by credits/free tier). This is why the default budget is
  **$10**, not $1. On a new Free plan the $200 credits cover it; on a
  legacy account the 30 GiB EBS allowance may still apply, but public
  IPv4 is billed. Check **Billing → Free Tier** for your account.
- The instance's public IPv4 is ephemeral: it changes after
  stop/start (including the one-time stop/start when changing
  `disable_api_termination`). Re-read `terraform output public_ip`.
- `disable_api_termination = true` blocks accidental EC2 termination.
  To destroy intentionally, first disable protection:

  ```powershell
  ..\..\.tools\terraform\terraform.exe apply -var="protect_instance=false"
  ..\..\.tools\terraform\terraform.exe destroy
  ```

- **End of trial:** 2026-12-31. Plan the move to Oracle (permanent) or
  GitHub ARM runners before then; this stack is disposable by design.

## Teardown

`terraform destroy` intentionally fails while the S3 bucket contains
objects (`force_destroy = false`) and while termination protection is
on. Export/delete evidence and set `protect_instance=false` first if
you really mean it.

## State

State is in the versioned, encrypted, TLS-only S3 bucket
`eval-tfstate-<account>-<region>` under
`eval-runner/terraform.tfstate`, with S3-native lockfiles. The
bootstrap stack keeps its own tiny local state. Never commit state.

## Terraform CI

`.github/workflows/terraform.yml` runs `terraform fmt -check` and
`terraform validate` for both stacks on every push/PR with no AWS
credentials. `.github/workflows/terraform-plan.yml` runs a read-only
plan on `main`/manual using GitHub OIDC and the `eval-github-plan` role;
`.github/workflows/terraform-drift.yml` runs the same plan daily. Apply
remains local/manual. See `docs/10-cicd-aws.md`.
