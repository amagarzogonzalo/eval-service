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
.\.tools\awscli\Scripts\aws.cmd configure
# Access key, secret, region: eu-north-1, output: json
.\.tools\awscli\Scripts\aws.cmd sts get-caller-identity
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

The main stack already points at that bucket. Then:

```powershell
..\..\.tools\terraform\terraform.exe init -migrate-state -force-copy
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

Build with 2 jobs (2 GiB RAM, 4 GiB swap):

```bash
sudo -u eval bash -lc 'cd /srv/eval-service && CARGO_BUILD_JOBS=2 cargo build --release -p eval-cli'
```

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
EVAL_PYTHON=python3 cargo test --workspace --locked
./target/release/eval check integrations/digest_agent/live_bench.yaml --probe
```

Then the live cycle through `eval proxy` with `--workers 1`, and a
cassette replay. Keep the suite out of CI.

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
`terraform validate` for both stacks on every push/PR. No AWS
credentials are used in CI; planning is a local, credentialed step.
