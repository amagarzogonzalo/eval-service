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

## 3. Plan and apply

```powershell
..\..\.tools\terraform\terraform.exe init
..\..\.tools\terraform\terraform.exe plan -out tfplan
..\..\.tools\terraform\terraform.exe apply tfplan
```

This creates: VPC + public subnet + IGW, security group, IAM role +
instance profile (SSM, S3 store, `/eval/*` parameters), `t4g.small`
instance, 30 GiB encrypted gp3 boot volume, versioned S3 store, and a
$1 monthly budget with email alerts. Confirm the budget subscription
email once.

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

## Costs and guardrails

- **Compute:** `t4g.small` hours are free until **2026-12-31** (750
  h/month); CPU credit mode is set to `standard` to avoid surplus
  credit charges.
- **Not free:** EBS (~30 GiB gp3), the public IPv4 (~$0.005/h), and
  egress. These are small; the $1 budget catches runaway spend. On a
  new account they consume Free-plan credits; on a legacy account the
  30 GiB EBS allowance may still apply.
- The instance's public IPv4 is ephemeral: it changes after
  stop/start. Re-read `terraform output public_ip` and update your SSH
  known-hosts entry.
- **End of trial:** 2026-12-31. Plan the move to Oracle (permanent) or
  GitHub ARM runners before then; this stack is disposable by design.

## Teardown

```powershell
..\..\.tools\terraform\terraform.exe destroy
```

`terraform destroy` intentionally fails while the S3 bucket contains
objects (`force_destroy = false`): export or delete the evidence first
if you really mean it.

## State

Terraform state is local (`terraform.tfstate`, gitignored). Back it up
before re-imaging the workstation, or migrate to an S3 backend once
the account is stable. Never commit it.
