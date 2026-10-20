# CI/CD for Terraform and AWS

Status: **PR A implements the OIDC plan lane; apply remains deferred**.
The OIDC provider and `eval-github-plan` role exist in
`infra/aws/bootstrap/`. `terraform-plan.yml` and `terraform-drift.yml`
are the only AWS-authenticated workflows. No apply role or apply
workflow has been created.

Owner: repository owner. Last updated: 2026-10-20.

## 1. Decision summary

- Authenticate GitHub Actions to AWS with **OIDC federation**, never with
  long-lived `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` secrets.
- Split credentials by purpose: a **read-only plan role** now. A separate
  **apply role** is deferred until the plan lane has produced trustworthy
  evidence. No role is shared between plan and apply.
- Keep three implemented workflows:
  **validate** (no AWS), **plan** (read-only AWS), **drift** (read-only,
  scheduled). Apply is a future, manual lane.
- Plan on `main`/manual only. Do not run state-reading plans for
  untrusted pull requests.
- Apply remains **manual and local** until a separate apply role and
  protected environment are implemented. Apply never runs automatically
  on push.
- Put the GitHub OIDC provider and the plan role in the bootstrap stack,
  because that stack is already the trust root for remote state.
- Scope trust with the repository's **immutable subject claims**. This
  repository was created on 2026-10-20, so GitHub's post-2026-07-15
  format applies (see section 5.3).

## 2. Current state

What exists today:

- `ci-fast.yml`: Rust tests on Ubuntu, Windows and ARM64, MSRV 1.85, offline
  tests, Python lint, gitleaks. It uses `make verify` on Linux.
- `terraform.yml`: `terraform fmt -check`, `init -backend=false`,
  `validate` and cloud-init schema validation. No AWS credentials.
- Terraform state is remote in S3 with native lockfiles;
  `infra/aws/bootstrap` creates the state bucket using local state.
- Apply is manual and local, using an IAM user with administrator
  access. This is the break-glass path, not the target path.
- Provider lockfiles now carry checksums for Linux, macOS and Windows
  on amd64 and arm64. This was required to make Linux CI pass.

Implemented in PR A:

- GitHub OIDC provider and `eval-github-plan` role in
  `infra/aws/bootstrap/oidc.tf`.
- `terraform-plan.yml` on `main`/manual and `terraform-drift.yml` on a
  daily schedule. Both use OIDC, pinned Actions and the plan role.
- The plan role can read state and write only
  `eval-runner/terraform.tfstate.tflock`.

Known gaps:

- No apply role and no apply workflow.
- Drift currently fails the workflow; it does not yet open/update a
  GitHub issue.
- The SNS email subscription is still `PendingConfirmation`.
- The live provider key exists only in SSM
  (`/eval/openrouter_api_key`), never in GitHub secrets.
- The Makefile, CI wiring and multi-platform lockfiles are committed.
- The main-stack backend bucket is supplied at init time with
  `-backend-config="bucket=..."`; only `infra/aws/bootstrap` still uses
  local state.
- This document and the plan lane are under review in PR A.

## 3. Goals and non-goals

Goals:

- No long-lived AWS credentials in GitHub.
- A PR cannot modify infrastructure.
- A push to `main` cannot apply infrastructure.
- Every apply is preceded by a plan produced from the same commit.
- Drift is detected without human intervention.
- Terraform state remains readable only by the roles that need it.
- The pipeline is reproducible locally (`make tf-validate`) and on the
  runner.

Non-goals for this phase:

- Multi-account AWS organization. This is a single-account setup.
- Automatic apply on merge.
- Running live LLM evaluations in CI. The runner owns those; CI does
  not hold provider keys.
- Migrating to HCP Terraform/Atlantis. That is a valid later option, not
  a prerequisite.
- Managing application releases (Docker images, binaries) here.

## 4. Target architecture

```text
                     GitHub Actions (OIDC JWT)
                              |
                              |  sts:AssumeRoleWithWebIdentity
                              v
        +---------------------+----------------------+
        |                                            |
  eval-github-plan                          eval-github-apply (future)
  (read-only, main/manual)                  (privileged, production env)
        |                                            |
        |  terraform plan (state lock enabled)       |  terraform apply
        v                                            v
  +--------------------------------------------------------+
  |  AWS account <account-id> / eu-north-1                 |
  |                                                        |
  |  S3 state bucket  <---------------------->  Terraform |
  |  (versioned, encrypted, private)                       |
  |                                                        |
  |  VPC, EC2 t4g.small, S3 eval store, IAM, SNS,          |
  |  CloudWatch alarms, AWS Budgets                        |
  +--------------------------------------------------------+
```

Trust boundaries:

| Boundary | Rule |
|---|---|
| Pull requests | No AWS credentials, no state access |
| Plan/drift workflow | Read-only role, no state-object writes, lockfile writes only |
| Apply workflow (future) | Apply role, manual dispatch, protected environment or manual gate |
| Local admin | Bootstrap and break-glass only; rotate after OIDC works |
| Runner instance | Instance role for SSM/S3; no GitHub credentials |

## 5. AWS design

### 5.1 OIDC provider

Create one provider for GitHub:

- URL: `https://token.actions.githubusercontent.com`
- Audience: `sts.amazonaws.com`
- Thumbprint: obtained from GitHub's certificate chain, not hard-coded.

Terraform belongs in `infra/aws/bootstrap/` (new `oidc.tf`), because the
bootstrap stack is the trust root. Add the `hashicorp/tls` provider.

```hcl
data "tls_certificate" "github" {
  url = "https://token.actions.githubusercontent.com/.well-known/openid-configuration"
}

resource "aws_iam_openid_connect_provider" "github" {
  url             = "https://token.actions.githubusercontent.com"
  client_id_list  = ["sts.amazonaws.com"]
  thumbprint_list = [data.tls_certificate.github.certificates[0].sha1_fingerprint]
}
```

Preflight: run `aws iam list-open-id-connect-providers` first. If a
GitHub provider already exists in the account, import it into the
bootstrap state instead of creating a second one.

Expose outputs:

```hcl
output "github_oidc_provider_arn" {
  value = aws_iam_openid_connect_provider.github.arn
}

output "github_plan_role_arn" {
  value = aws_iam_role.github_plan.arn
}
```

### 5.2 Role matrix

| Role | Assumed by | Can do | Cannot do |
|---|---|---|---|
| `eval-github-plan` | `main` / manual plan and drift workflows | Read resources; read state; create/delete the state `.tflock` | Write the state object, change AWS resources |
| `eval-github-apply` (future) | protected `production` environment, manual dispatch | Plan + apply the stack; write state/lock | Assume other roles; touch unrelated accounts |
| local `Alejandro` user | local bootstrap/break-glass | Admin | Not used by CI |

Implemented role name:

- `eval-github-plan`

Future role name (not created):

- `eval-github-apply`

### 5.3 Trust policies (exact subject claims)

This repository is public, created **2026-10-20**, so GitHub uses the
immutable subject format:

```text
repo:OWNER@OWNER_ID/REPO@REPO_ID:...
```

For this repository:

- Owner ID: `132710511`
- Repository ID: `1426901614`
- Repository: `amagarzogonzalo/eval-service`

Plan role trust policy:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Principal": {
        "Federated": "arn:aws:iam::<account-id>:oidc-provider/token.actions.githubusercontent.com"
      },
      "Action": "sts:AssumeRoleWithWebIdentity",
      "Condition": {
        "StringEquals": {
          "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
          "token.actions.githubusercontent.com:sub": [
            "repo:amagarzogonzalo@132710511/eval-service@1426901614:ref:refs/heads/main"
          ]
        }
      }
    }
  ]
}
```

The implemented plan role is `eval-github-plan`. Its trust policy has no
wildcards and no `pull_request`, fork, environment or tag subject: only
`ref:refs/heads/main`. There is no apply role or apply trust policy in
the account yet.

Future apply role trust policy (not created in PR A; environment-scoped):

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Principal": {
        "Federated": "arn:aws:iam::<account-id>:oidc-provider/token.actions.githubusercontent.com"
      },
      "Action": "sts:AssumeRoleWithWebIdentity",
      "Condition": {
        "StringEquals": {
          "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
          "token.actions.githubusercontent.com:sub": "repo:amagarzogonzalo@132710511/eval-service@1426901614:environment:production"
        }
      }
    }
  ]
}
```

If a GitHub environment is not used (see section 6.6), replace the apply
subject with the `ref:refs/heads/main` form. Do not allow both a
`pull_request` subject and state access.

Before applying these policies, run the OIDC debugger once on `main` and
compare the `sub` claim. The IDs above are stable, but the check is cheap
and prevents a trust-policy lockout. If the token shows the legacy
format without `@owner_id`/`@repo_id`, either opt the repository into
immutable claims or adjust the trust policy; do not mix both formats.

### 5.4 Plan role permissions

Minimum useful policy:

- `ReadOnlyAccess` managed policy, for resource discovery.
- Explicit state-bucket read (defense in depth):
  - `s3:GetBucketLocation` and `s3:ListBucket` on the state bucket.
  - `s3:GetObject` on `eval-runner/*` (state and lock objects).
- Lockfile writes, required because `use_lockfile = true`:
  - `s3:PutObject` and `s3:DeleteObject` on
    `eval-runner/terraform.tfstate.tflock` only.
- No `s3:PutObject` and no `s3:DeleteObject` on the state object.
- Plan runs with state locking enabled (`-lock-timeout=5m`), so a plan
  is not literally read-only: it creates and removes the `.tflock`.
- `sts:GetCallerIdentity` is allowed by `ReadOnlyAccess` via IAM; verify
  with the first run.
- Session duration: 1 hour or less.

`ReadOnlyAccess` is broad. For a high-value account, replace it with a
curated read-only policy for EC2, S3, IAM, SNS, CloudWatch, Budgets and
SSM. This project stays in one account, so the managed policy is an
accepted trade-off and is documented as such.

### 5.5 Apply role permissions

Start with the exact state-bucket access Terraform needs:

- `s3:ListBucket` on the bucket.
- `s3:GetObject` and `s3:PutObject` on the state object.
- `s3:GetObject`, `s3:PutObject`, `s3:DeleteObject` on the `.tflock`
  object.
- No `s3:DeleteObject` on the state object.

For AWS resources, do not attach `AdministratorAccess`. Two acceptable
ways to build the policy:

1. **Curated policy (recommended).** Enumerate the create/read/update/
   delete actions for the services in `main.tf` (EC2, S3 store, IAM
   role/policy/profile, SNS, CloudWatch, Budgets), scoped to the
   `eval-runner-*` names and the evaluation bucket. Include `iam:PassRole`
   only for the runner role.
2. **Bootstrap from CloudTrail.** Apply the stack once manually, generate
   an IAM policy from the operator's CloudTrail events with IAM Access
   Analyzer, remove admin-only actions, then attach it. Repeat after any
   resource-type addition.

Add a permissions boundary to the apply role so a compromised workflow
cannot create arbitrary IAM principals.

### 5.6 State bucket hardening

Already present: versioning, default encryption, public-access block,
TLS-only bucket policy, ownership controls, native lockfile.

Add:

- Lifecycle rule for non-current versions: keep the newest 10, expire
  after 90 days. Versioning remains the recovery mechanism; lifecycle
  only bounds cost.
- Optional KMS CMK if state is judged sensitive. The state currently
  contains infrastructure metadata and the alert email, not provider
  keys. SSE-S3 is acceptable for phase 3; record the decision.
- CloudTrail is enabled at the account level (verify), so
  `AssumeRoleWithWebIdentity` and S3 state writes are attributable.
- Do not grant the plan role write access to the state object; it may
  write only `eval-runner/terraform.tfstate.tflock`.

## 6. GitHub design

### 6.1 Workflow matrix

| Workflow | Trigger | AWS role | Environment | Approval |
|---|---|---|---|---|
| `ci-fast.yml` | push, PR | none | none | none |
| `terraform.yml` (validate) | push, PR | none | none | none |
| `terraform-plan.yml` | push to `main`, manual | plan | none | none |
| `terraform-apply.yml` (future) | manual only | apply | `production` | reviewer if available; otherwise manual dispatch |
| `terraform-drift.yml` | daily schedule, manual | plan | none | none |

`terraform-plan.yml` and `terraform-drift.yml` pin Actions to full commit
SHAs and run only on `refs/heads/main`. The trust policy rejects every
other ref, so a manual dispatch against a branch fails closed.

### 6.2 Validate

`terraform.yml` is credential-free and runs fmt, cloud-init schema,
`init -backend=false`, validate and the Makefile dry checks. Its Actions
are pinned to full SHAs.

Add optional static analysis later (section 9, phase 4): `tflint`,
`trivy config`/`checkov`, `actionlint`, `shellcheck`.

### 6.3 Plan workflow

The implemented workflow is `.github/workflows/terraform-plan.yml`. Its
security shape:

```yaml
name: terraform-plan
on:
  workflow_dispatch:
  push:
    branches: [main]
    paths:
      - "infra/aws/**"
      - ".github/workflows/terraform-plan.yml"

permissions:
  contents: read
  id-token: write

jobs:
  plan:
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    env:
      TF_STATE_BUCKET: ${{ vars.TF_STATE_BUCKET }}
      TF_VAR_admin_cidr: ${{ vars.TF_VAR_ADMIN_CIDR }}
      TF_VAR_alert_email: ${{ vars.TF_VAR_ALERT_EMAIL }}
      TF_VAR_budget_usd: ${{ vars.TF_VAR_BUDGET_USD }}
    steps:
      - uses: actions/checkout@<sha>
        with:
          persist-credentials: false
      - uses: aws-actions/configure-aws-credentials@<sha>
        with:
          role-to-assume: ${{ vars.AWS_PLAN_ROLE_ARN }}
          role-session-name: gha-terraform-plan-${{ github.run_id }}
          aws-region: ${{ vars.AWS_REGION }}
      - uses: hashicorp/setup-terraform@<sha>
        with:
          terraform_version: "1.16.6"
          terraform_wrapper: false
      - run: |-
          terraform -chdir=infra/aws init -input=false -lockfile=readonly \
            -backend-config="bucket=${TF_STATE_BUCKET}"
          terraform -chdir=infra/aws plan -input=false -no-color -lock-timeout=5m
```

Requirements:

- `AWS_REGION`, `TF_STATE_BUCKET`, `TF_VAR_ADMIN_CIDR`,
  `TF_VAR_ALERT_EMAIL`, `TF_VAR_BUDGET_USD` and
  `EVAL_RUNNER_SSH_PUBLIC_KEY` are repository variables. No AWS keys are
  stored in GitHub.
- The workflow writes the public SSH key to
  `~/.ssh/eval-runner-aws.pub` before planning, because the main stack
  accepts a key path.
- `init -lockfile=readonly` must not modify the provider lockfile.
- The state lock is enabled. The role may create/delete only the
  `.tflock`; the plan is not permitted to write the state object.
- Do not upload a saved plan file. A plan file embeds sensitive values.
- Plan output stays in the Actions log, not a PR artifact.
- A plan that shows changes is not a failure on `main`; it is expected
  after a merge. Fail only on Terraform errors.

### 6.4 Apply workflow (future, not implemented)

No apply role or workflow exists. Recommended shape when it is added:
one manual job, plan and apply in the same job, no saved-plan artifact.

```yaml
name: terraform-apply
on:
  workflow_dispatch:
    inputs:
      confirm:
        description: Type "apply" to confirm
        required: true
        type: string

permissions:
  contents: read
  id-token: write

concurrency:
  group: terraform-apply
  cancel-in-progress: false

jobs:
  apply:
    if: inputs.confirm == 'apply'
    runs-on: ubuntu-latest
    environment: production
    steps:
      - uses: actions/checkout@<sha>
      - uses: aws-actions/configure-aws-credentials@<sha>
        with:
          role-to-assume: ${{ vars.AWS_APPLY_ROLE_ARN }}
          role-session-name: gha-terraform-apply
          aws-region: eu-north-1
      - uses: hashicorp/setup-terraform@<sha>
        with:
          terraform_version: "1.16.6"
          terraform_wrapper: false
      - run: >-
          terraform -chdir=infra/aws init -input=false -lockfile=readonly
          -backend-config="bucket=${{ vars.TF_STATE_BUCKET }}"
      - run: terraform -chdir=infra/aws plan -input=false -lock-timeout=5m -out=tfplan
      - run: terraform -chdir=infra/aws apply -input=false -lock-timeout=5m tfplan
      - name: Delete plan file
        if: always()
        run: rm -f infra/aws/tfplan
```

Why plan and apply in one job:

- A saved plan is a sensitive artifact containing full planned values.
- Uploading it to Actions artifacts widens access and retention.
- Applying the exact saved plan in the same job avoids plan/apply drift.

The trade-off is that the reviewer does not see the plan before the
environment approval. If that matters, add a separate plan job that
publishes a redacted summary, then approve the apply job; still do not
publish the plan binary.

### 6.5 Drift workflow

```yaml
name: terraform-drift
on:
  schedule:
    - cron: "17 6 * * *"
  workflow_dispatch:

permissions:
  contents: read
  id-token: write
  issues: write

concurrency:
  group: terraform-drift
  cancel-in-progress: false
```

Steps:

1. Assume the **plan** role.
2. `terraform init -input=false -lockfile=readonly
   -backend-config="bucket=$TF_STATE_BUCKET"`
3. `terraform plan -detailed-exitcode -lock-timeout=5m -no-color`
   - `0` = no drift.
   - `1` = Terraform error, fail the job.
   - `2` = drift or pending change, fail the workflow.
4. Do not apply. The failed run links to the commit; the next manual
   apply resolves it.
5. Opening/closing a `terraform-drift` issue is a future enhancement;
   the current workflow relies on the Actions failure notification.

On `main`, an expected pending change after a merge is indistinguishable
from drift. Treat the failure as "pending change or drift" rather than
claiming manual modification. The future apply workflow is the resolver.

### 6.6 Variables, environments, and approval fallback

Repository variables (not secrets) used by the implemented plan lane:

- `AWS_PLAN_ROLE_ARN`
- `AWS_REGION`
- `TF_STATE_BUCKET` (for example
  `eval-tfstate-<account-id>-eu-north-1`)
- `TF_VAR_admin_cidr`
- `TF_VAR_alert_email`
- `TF_VAR_budget_usd`
- `EVAL_RUNNER_SSH_PUBLIC_KEY`

`AWS_APPLY_ROLE_ARN` is reserved for the future apply lane and is not
set. Terraform variables that are secrets must not be stored in the
repo or in `.tfvars`. The current stack has no AWS secrets in state; it
reads the provider key from SSM at job time on the runner.

Environment:

- Plan and drift use no GitHub environment.
- The future apply role should use `production`, restricted to `main`,
  with required reviewers if the plan supports them.
- Store nothing in the environment unless it is needed by apply.

Important limitation: GitHub Free/Pro/Team may not offer required
reviewers or wait timers for private repositories. This repository is
public; verify the environment policy before relying on it for apply.

Fallback if required reviewers are unavailable:

- Keep `terraform-apply.yml` **workflow_dispatch only** on `main`.
- Remove `environment:` from the apply job and scope the trust policy to
  `ref:refs/heads/main`; an environment-scoped job emits an environment
  subject, not the ref.
- Require a pull request review and a green `terraform-plan` run before
  dispatching apply. This is process enforcement, not a hard gate.
- If a hard gate is mandatory, either upgrade to a plan that supports
  protected private environments or keep apply local/manual.

### 6.7 Permissions and supply-chain hardening

- Workflow-level `permissions: {}` or `contents: read`; grant
  `id-token: write` only on plan/apply/drift jobs.
- Pin third-party Actions to full commit SHAs; use Dependabot/Renovate
  to update pins. Tags are mutable.
- Never use `pull_request_target` with a checkout of untrusted code.
- Set `persist-credentials: false` on checkout for AWS jobs if the
  workflow does not push.
- Prefer `AWS_REGION`/`aws-region` over hard-coded regional endpoints in
  scripts.
- Do not echo OIDC tokens, plan files, state, or SSM values.

## 7. Terraform changes

### 7.1 Bootstrap stack

Implemented in `infra/aws/bootstrap/oidc.tf`:

- `hashicorp/tls` provider.
- OIDC provider with a dynamic root thumbprint.
- `eval-github-plan` role and lockfile-scoped inline policy.
- Outputs for the provider ARN and plan role ARN.

The apply role is not implemented.

Bootstrap remains the only stack applied with local admin. After the
first successful OIDC plan, optionally migrate bootstrap state into S3:

1. Add a `backend "s3"` block pointing at the state bucket it created,
   key `bootstrap/terraform.tfstate`.
2. `terraform init -migrate-state`.
3. Commit the backend block; do not commit local state.

This removes the last local-state copy from the normal workflow. Keep a
local `terraform.tfstate` backup until the migration is verified.

### 7.2 Main stack

No main-stack code changes were required for PR A. The plan workflow
writes `EVAL_RUNNER_SSH_PUBLIC_KEY` to
`~/.ssh/eval-runner-aws.pub`, matching the existing
`ssh_public_key_path` input. A future cleanup can replace the path-only
input with an `ssh_public_key` string variable, but that is not part of
the OIDC lane.

- Keep `use_lockfile = true` and the pinned provider constraints.
- Do not add modules or workspaces for this single-environment stack.

### 7.3 Lockfile and version policy

- Keep multi-platform hashes in both `.terraform.lock.hcl` files.
- CI must run `terraform init -lockfile=readonly` so a missing hash is a
  hard failure.
- Add a CI check that regeneration does not change the lockfile:
  `terraform providers lock -platform=linux_amd64 ...` followed by
  `git diff --exit-code`.
- Keep Terraform pinned to `1.16.6` in workflows and `>= 1.10.0` in
  configuration. Upgrade deliberately.
- Pass the state bucket at init (`-backend-config="bucket=..."`); keep
  its value in the `TF_STATE_BUCKET` repository variable.

## 8. Security controls checklist

| Control | Target |
|---|---|
| No static AWS keys in GitHub | Required |
| OIDC `sub` scoped to repo + `refs/heads/main` only | Required |
| Plan role cannot write the state object | Required |
| Plan role can write only the state `.tflock` | Required |
| Apply role cannot administer the account | Future |
| Manual approval before apply | Future |
| Branch protection / rulesets on `main` | Required |
| Required checks: `rust (ubuntu-latest)`, `rust (windows-latest)`, `arm`, `msrv`, `offline`, `python-lint`, `secret-scan`, `fmt-validate`, `pr-gate` | Required |
| CODEOWNERS for `infra/**` and `.github/**` | Required |
| Actions pinned to SHAs | Required |
| State encrypted, versioned, private | Required |
| State bucket lifecycle for old versions | Required |
| CloudTrail enabled and retained | Required (verify) |
| SNS subscription confirmed | Required |
| Budget + stop action tested | Required |
| Plan role used for drift | Required |
| No live LLM keys in CI | Required |

### 8.1 GitHub settings applied

On 2026-10-20 the repository was configured with:

- Branch protection on `main`: pull requests required, strict required
  checks, conversation resolution, no force pushes, no deletions, and
  administrators enforced.
- Required checks: `rust (ubuntu-latest)`, `rust (windows-latest)`,
  `arm`, `msrv`, `offline`, `python-lint`, `secret-scan`,
  `fmt-validate`, `pr-gate`.
- Actions: selected allowlist (GitHub-owned, verified, and
  `dtolnay/rust-toolchain`, `hashicorp/setup-terraform`,
  `aws-actions/configure-aws-credentials`), with full-SHA pinning
  required.
- Dependabot alerts and security updates enabled; weekly
  `github-actions` updates configured in `.github/dependabot.yml`.
- `pr-gate` required: replay-only offline suite, baseline from `main`,
  candidate in the Phase 4 sandbox, no secrets or live provider keys.
- Secret scanning and push protection enabled. GitHub's API still
  reports non-provider patterns and validity checks as disabled for this
  repository; that capability gap is recorded rather than claimed.

## 9. Rollout plan

### Phase 0 — make the current CI trustworthy

- Commit the Makefile, CI, lockfile and docs changes.
- Push and confirm `ci-fast` and `terraform` are green.
- Pull the runner to the same commit.

Acceptance: both workflows green on `main`; no AWS credentials in
GitHub; `make tf-validate` green on Linux.

### Phase 1 — OIDC plan lane

- Add OIDC provider and plan role to bootstrap.
- Apply bootstrap locally.
- Run the OIDC debugger on `main`; confirm the immutable `sub`.
- Add `terraform-plan.yml`.
- Set `AWS_PLAN_ROLE_ARN`, `TF_VAR_*` and the SSH public key.
- Run plan manually; compare with a local `terraform plan`.

Acceptance: plan workflow succeeds with the state lock enabled; it can
read the state object; it cannot write the state object (only the
`.tflock`); no secrets appear in logs; role session appears in
CloudTrail.

### Phase 2 — drift

- Add `terraform-drift.yml`.
- Test by changing a harmless tag on the EC2 instance, running the
  workflow, confirming the failure, then reverting through Terraform.
- Confirm the next run exits 0.

Acceptance: drift detected and reported as a failed run; no apply
performed.

### Phase 3 — apply (optional)

- Add the apply role and `terraform-apply.yml`.
- Create the `production` environment; enable protection if available.
- Run one no-op apply and one real change.
- Rotate the local admin access key after success; keep break-glass
  credentials offline/with MFA.

Acceptance: apply only from a manual dispatch; plan and apply use the
same commit; state lock prevents concurrent applies; no static keys.

### Phase 4 — hardening

- Actions are pinned to SHAs and Dependabot is enabled for
  `github-actions`.
- Add `tflint` or `trivy config`/`checkov`.
- Add `actionlint` and `shellcheck` for workflow/embedded scripts.
- Consider KMS CMK for state if sensitivity changes.
- Consider OIDC + plan for a separate `dev` account if the project grows.

## 10. Verification checklist

Run these before declaring the design implemented.

AWS:

```sh
aws iam list-open-id-connect-providers
aws iam get-role --role-name eval-github-plan
aws s3api get-bucket-versioning --bucket eval-tfstate-<account-id>-eu-north-1
aws cloudtrail lookup-events \
  --lookup-attributes AttributeKey=EventName,AttributeValue=AssumeRoleWithWebIdentity
```

GitHub:

```sh
gh variable list
gh secret list                 # no AWS keys
gh workflow run terraform-plan.yml
gh run list --workflow=terraform-plan.yml
```

Terraform:

```sh
make tf-validate
terraform -chdir=infra/aws init -input=false -lockfile=readonly \
  -backend-config="bucket=$TF_STATE_BUCKET"
terraform -chdir=infra/aws plan -detailed-exitcode -lock-timeout=5m
```

Expected:

- `list-open-id-connect-providers` contains GitHub.
- The plan role can read the state object; a plan succeeds with the
  state lock enabled.
- The plan role cannot `s3:PutObject` or delete the state object; it can
  write only `eval-runner/terraform.tfstate.tflock`.
- No apply role is assumed because none exists.
- Drift plan exits `0` when clean, `2` when changed, `1` on error; the
  workflow fails on `2`.
- No plan file is uploaded as an artifact.
- No workflow logs contain `Authorization`, tokens, or SSM values.

## 11. Risks and mitigations

| Risk | Mitigation |
|---|---|
| OIDC trust too broad | Immutable `sub`, `refs/heads/main` only, no PR/fork state access |
| Plan role can read all account data | Accept for single account; account separation later |
| Apply role over-privileged | Deferred; curated policy, permissions boundary, manual gate |
| Plan/apply divergence | Future apply uses the saved plan in the same job |
| Saved plan leaks | No artifact upload; delete on success/failure |
| State leak | Private bucket, TLS, encryption, versioning, restricted roles |
| Workflow supply chain | Pin Actions to SHAs; least `permissions` |
| GitHub plan lacks approval gate | Apply remains local/manual; plan lane does not need approval |
| Lockfile portability | Multi-platform lockfile + `-lockfile=readonly` CI check |
| Local admin key compromise | MFA, rotate after OIDC, keep only for bootstrap |
| Drift failure noise after merge | Treat as "pending change or drift"; resolve via manual apply |
| Costs | OIDC/IAM free; state storage negligible; Actions minutes bounded |

## 12. Decisions and open items

Accepted for PR A:

- OIDC-only authentication; no AWS keys in GitHub.
- Plan/drift on `main`/manual only; no PR plans.
- Plan role may lock state but may not write the state object.
- Apply stays local/manual; no apply role or workflow yet.
- KMS CMK deferred; SSE-S3 accepted for state.
- Bootstrap state remains local until the plan lane is proven.

Still open:

- Apply role, protected environment and manual approval path.
- Opening/closing a `terraform-drift` issue.
- Bootstrap state migration to S3.
- Multi-platform lockfile regeneration check.

## 13. References

- GitHub OIDC in AWS:
  https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-aws
- GitHub OIDC reference and immutable subject claims:
  https://docs.github.com/en/actions/reference/openid-connect-reference
- GitHub Actions security hardening:
  https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions
- GitHub environments:
  https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments
- `aws-actions/configure-aws-credentials`:
  https://github.com/aws-actions/configure-aws-credentials
- Terraform S3 backend:
  https://developer.hashicorp.com/terraform/language/backend/s3
- Terraform sensitive data and plan files:
  https://developer.hashicorp.com/terraform/language/manage-sensitive-data
  https://developer.hashicorp.com/terraform/cli/commands/plan
- AWS Prescriptive Guidance, Terraform backend best practices:
  https://docs.aws.amazon.com/prescriptive-guidance/latest/terraform-aws-provider-best-practices/backend.html
- HashiCorp Terraform security practices:
  https://www.hashicorp.com/en/blog/terraform-security-5-foundational-practices
- OWASP Non-Human Identities Top 10 (2025), insecure cloud deployment:
  https://owasp.org/www-project-non-human-identities-top-10/2025/6-insecure-cloud-deployment-configurations
