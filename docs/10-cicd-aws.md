# CI/CD for Terraform and AWS (proposed)

Status: **proposed, not implemented**. This document is the review gate:
nothing in sections 4–8 is applied until the decisions in section 12 are
accepted.

Owner: repository owner. Last updated: 2026-10-20.

## 1. Decision summary

- Authenticate GitHub Actions to AWS with **OIDC federation**, never with
  long-lived `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` secrets.
- Split credentials by purpose: a **read-only plan role** and a separate
  **apply role**. No role is shared between plan and apply.
- Keep four independent workflows:
  **validate** (no AWS), **plan** (read-only AWS), **apply** (privileged,
  manual), **drift** (read-only, scheduled).
- Plan on `main`/manual first. Do not run state-reading plans for
  untrusted pull requests.
- Apply remains **manual** until the plan lane has produced trustworthy
  evidence for a few changes. Apply never runs automatically on push.
- Put the GitHub OIDC provider and both roles in the bootstrap stack,
  because that stack is already the trust root for remote state.
- Scope trust with the repository's **immutable subject claims**. This
  repository was created on 2026-10-14, so GitHub's post-2026-07-15
  format applies (see section 5.3).

## 2. Current state

What exists today:

- `ci-fast.yml`: Rust tests on Ubuntu and Windows, MSRV 1.85, offline
  tests, Python lint, gitleaks. It uses `make verify` on Linux.
- `terraform.yml`: `terraform fmt -check`, `init -backend=false`,
  `validate` and cloud-init schema validation. No AWS credentials.
- Terraform state is remote in S3 with native lockfiles;
  `infra/aws/bootstrap` creates the state bucket using local state.
- Apply is manual and local, using an IAM user with administrator
  access. This is the break-glass path, not the target path.
- Provider lockfiles now carry checksums for Linux, macOS and Windows
  on amd64 and arm64. This was required to make Linux CI pass.

Known gaps:

- No AWS authentication and no plan/apply workflow.
- No drift detection.
- The SNS email subscription is still `PendingConfirmation`.
- No provider key exists under `/eval/*` in SSM yet.
- The Makefile, CI wiring and multi-platform lockfiles are committed.
- The main-stack backend bucket is supplied at init time with
  `-backend-config="bucket=..."`; only `infra/aws/bootstrap` still uses
  local state.
- This design document is the review gate; no OIDC role or workflow has
  been applied yet.

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
  eval-github-plan                          eval-github-apply
  (read-only, main/manual)                  (privileged, production env)
        |                                            |
        |  terraform plan -lock=false                |  terraform apply
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
| Plan workflow | Read-only role, no state writes, `-lock=false` |
| Apply workflow | Apply role, manual dispatch, protected environment or manual gate |
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

output "github_apply_role_arn" {
  value = aws_iam_role.github_apply.arn
}
```

### 5.2 Role matrix

| Role | Assumed by | Can do | Cannot do |
|---|---|---|---|
| `eval-github-plan` | `main` / manual plan workflow | Read resources; read state | Write state, change AWS resources |
| `eval-github-apply` | protected `production` environment, manual dispatch | Plan + apply the stack; write state/lock | Assume other roles; touch unrelated accounts |
| local `Alejandro` user | local bootstrap/break-glass | Admin | Not used by CI |

Recommended role names:

- `eval-github-plan`
- `eval-github-apply`

### 5.3 Trust policies (exact subject claims)

This repository is private, created **2026-10-14**, so GitHub uses the
immutable subject format:

```text
repo:OWNER@OWNER_ID/REPO@REPO_ID:...
```

For this repository:

- Owner ID: `132710511`
- Repository ID: `1418642634`
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
            "repo:amagarzogonzalo@132710511/eval-service@1418642634:ref:refs/heads/main"
          ]
        }
      }
    }
  ]
}
```

Apply role trust policy (environment-scoped):

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
          "token.actions.githubusercontent.com:sub": "repo:amagarzogonzalo@132710511/eval-service@1418642634:environment:production"
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
  - `s3:GetObject` on `eval-runner/terraform.tfstate`.
- No `s3:PutObject`, no `s3:DeleteObject`.
- Use `terraform plan -lock=false`; plan must not write the lock file.
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
- Do not grant the plan role object-write access to the state bucket.

## 6. GitHub design

### 6.1 Workflow matrix

| Workflow | Trigger | AWS role | Environment | Approval |
|---|---|---|---|---|
| `ci-fast.yml` | push, PR | none | none | none |
| `terraform.yml` (validate) | push, PR | none | none | none |
| `terraform-plan.yml` | push to `main`, manual | plan | none | none |
| `terraform-apply.yml` | manual only | apply | `production` | reviewer if available; otherwise manual dispatch |
| `terraform-drift.yml` | daily schedule, manual | plan | none | none |

Plan and apply must check out the same commit. The apply run must print
and record the commit SHA in its summary.

### 6.2 Validate

Keep `terraform.yml` as-is: fmt, cloud-init schema, init without backend,
validate, Makefile dry checks. It must remain credential-free.

Add optional static analysis later (section 9, phase 4): `tflint`,
`trivy config`/`checkov`, `actionlint`, `shellcheck`.

### 6.3 Plan workflow

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

concurrency:
  group: terraform-plan-${{ github.ref }}
  cancel-in-progress: true

jobs:
  plan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<sha>
      - uses: aws-actions/configure-aws-credentials@<sha>
        with:
          role-to-assume: ${{ vars.AWS_PLAN_ROLE_ARN }}
          role-session-name: gha-terraform-plan
          aws-region: eu-north-1
      - uses: hashicorp/setup-terraform@<sha>
        with:
          terraform_version: "1.16.6"
          terraform_wrapper: false
      - run: >-
          terraform -chdir=infra/aws init -input=false -lockfile=readonly
          -backend-config="bucket=${{ vars.TF_STATE_BUCKET }}"
      - run: terraform -chdir=infra/aws plan -input=false -lock=false -no-color
```

Requirements:

- `TF_STATE_BUCKET` from a repository variable, passed to `init`.
- `TF_VAR_admin_cidr`, `TF_VAR_alert_email`, `TF_VAR_budget_usd` from
  repository variables.
- Write the SSH public key to `~/.ssh/eval-runner-aws.pub` from a
  variable/secret before planning (see section 7.2 for the better
  long-term fix).
- Do not upload a saved plan file. A plan file embeds sensitive values.
- Do not post raw plan output to a PR initially. Keep it in the private
  Actions log; add a summary comment later if the content is classified
  as non-sensitive.
- A plan that shows changes is not a failure on `main`; it is expected
  after a merge. Fail only on Terraform errors.

### 6.4 Apply workflow

Recommended shape: one manual job, plan and apply in the same job, no
saved-plan artifact.

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
3. `terraform plan -detailed-exitcode -lock=false -no-color`
   - `0` = no drift.
   - `1` = Terraform error, fail the job.
   - `2` = drift or pending change, open/update an issue labeled
     `terraform-drift`.
4. Do not apply. The issue links to the run; the next manual apply
   resolves it.
5. Close the issue automatically when a later drift run exits `0`.

On `main`, an expected pending change after a merge is indistinguishable
from drift. Label the issue as "pending change or drift" rather than
claiming manual modification. The apply workflow is the resolver.

### 6.6 Variables, environments, and approval fallback

Repository variables (not secrets):

- `AWS_PLAN_ROLE_ARN`
- `AWS_APPLY_ROLE_ARN`
- `TF_STATE_BUCKET` (for example
  `eval-tfstate-342940030437-eu-north-1`)
- `TF_VAR_admin_cidr`
- `TF_VAR_alert_email`
- `TF_VAR_budget_usd`
- `EVAL_RUNNER_SSH_PUBLIC_KEY`

Terraform variables that are secrets must not be stored in the repo or
in `.tfvars`. The current stack has no AWS secrets in state; it reads
the provider key from SSM at job time on the runner.

Environment:

- Create `production` in GitHub.
- Restrict deployment branches to `main`.
- Enable required reviewers **if the plan supports it**.
- Disable administrator bypass if available.
- Store nothing in the environment unless it is needed by apply.

Important plan limitation: GitHub Free/Pro/Team may not offer required
reviewers or wait timers for **private** repositories. Verify in
Settings → Environments before relying on it.

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

Add `oidc.tf` with:

- `hashicorp/tls` provider.
- OIDC provider.
- Plan role and inline policy.
- Apply role and inline policy or managed policy attachment.
- Outputs for both role ARNs.

Bootstrap remains the only stack applied with local admin. After the
first successful OIDC plan, optionally migrate bootstrap state into S3:

1. Add a `backend "s3"` block pointing at the state bucket it created,
   key `bootstrap/terraform.tfstate`.
2. `terraform init -migrate-state`.
3. Commit the backend block; do not commit local state.

This removes the last local-state copy from the normal workflow. Keep a
local `terraform.tfstate` backup until the migration is verified.

### 7.2 Main stack

Small, reviewable changes:

- Add `Environment = "prod"` to `default_tags`.
- Replace the file-only SSH key input with either:
  - a `ssh_public_key` string variable (CI sets `TF_VAR_ssh_public_key`),
    falling back to `ssh_public_key_path` when empty; or
  - keep the path and have CI write the key to that path.
  The first option is cleaner and avoids a fake home-directory file in
  CI. It is a small breaking change for local `terraform.tfvars`, so do
  it in the same PR that adds the plan workflow.
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
| OIDC `sub` scoped to repo + branch/environment | Required |
| Plan role cannot write state | Required |
| Apply role cannot administer the account | Required |
| Manual approval before apply | Required |
| Branch protection / rulesets on `main` | Required |
| Required checks: `ci-fast/rust`, `ci-fast/msrv`, `ci-fast/offline`, `ci-fast/python-lint`, `ci-fast/secret-scan`, `terraform/fmt-validate` | Required |
| CODEOWNERS for `infra/**` and `.github/workflows/**` | Required |

Some branch-protection and CODEOWNERS features are plan-dependent for
private repositories. If they are unavailable, record the gap and keep
apply manual; do not claim a hard gate that the plan cannot enforce.
| Actions pinned to SHAs | Required |
| State encrypted, versioned, private | Required |
| State bucket lifecycle for old versions | Required |
| CloudTrail enabled and retained | Required (verify) |
| SNS subscription confirmed | Required |
| Budget + stop action tested | Required |
| Plan role used for drift | Required |
| No live LLM keys in CI | Required |

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

Acceptance: plan workflow succeeds; it can read state; it cannot write
state; no secrets appear in logs; role session appears in CloudTrail.

### Phase 2 — drift

- Add `terraform-drift.yml`.
- Test by changing a harmless tag on the EC2 instance, running the
  workflow, confirming an issue, then reverting through Terraform.
- Close the issue on the next clean run.

Acceptance: drift detected and reported; no apply performed.

### Phase 3 — apply (optional)

- Add the apply role and `terraform-apply.yml`.
- Create the `production` environment; enable protection if available.
- Run one no-op apply and one real change.
- Rotate the local admin access key after success; keep break-glass
  credentials offline/with MFA.

Acceptance: apply only from a manual dispatch; plan and apply use the
same commit; state lock prevents concurrent applies; no static keys.

### Phase 4 — hardening

- Pin Actions to SHAs; enable Dependabot.
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
aws iam get-role --role-name eval-github-apply
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
terraform -chdir=infra/aws plan -detailed-exitcode -lock=false
```

Expected:

- `list-open-id-connect-providers` contains GitHub.
- Plan role can read state; a plan succeeds with `-lock=false`.
- Plan role cannot `s3:PutObject` or delete state.
- Apply role is assumed only from the protected environment/manual
  dispatch.
- Drift run exits `0` when clean, `2` when changed, `1` on error.
- No plan file is uploaded as an artifact.
- No workflow logs contain `Authorization`, tokens, or SSM values.

## 11. Risks and mitigations

| Risk | Mitigation |
|---|---|
| OIDC trust too broad | Immutable `sub`, branch/environment scope, no PR state access |
| Plan role can read all account data | Accept for single account; account separation later |
| Apply role over-privileged | Curated policy, permissions boundary, manual gate |
| Plan/apply divergence | Apply the saved plan in the same job |
| Saved plan leaks | No artifact upload; delete on success/failure |
| State leak | Private bucket, TLS, encryption, versioning, restricted roles |
| Workflow supply chain | Pin Actions to SHAs; least `permissions` |
| GitHub plan lacks approval gate | Fallback to workflow_dispatch on `main` or keep apply local |
| Lockfile portability | Multi-platform lockfile + `-lockfile=readonly` CI check |
| Local admin key compromise | MFA, rotate after OIDC, keep only for bootstrap |
| Drift issue noise after merge | Label "pending change or drift"; resolve via apply |
| Costs | OIDC/IAM free; state storage negligible; Actions minutes bounded |

## 12. Open decisions for review

1. **Approval availability.** Does the GitHub plan support required
   reviewers on this private repository? If not, accept the
   workflow_dispatch fallback or keep apply local.
2. **Apply in CI now or later?** Recommendation: later. Ship the plan
   and drift lanes first.
3. **PR plans?** Recommendation: no at first. Only `main`/manual. Enable
   PR plans after confirming state contents and trust boundaries.
4. **KMS CMK for state?** Recommendation: not now; revisit if secrets
   enter state.
5. **HCP Terraform/Atlantis?** Recommendation: no for this scale.
   GitHub Actions + OIDC + S3 backend is sufficient.
6. **Bootstrap state migration.** Recommendation: migrate to S3 after
   OIDC is proven, then remove local state.

### Review sign-off

- [ ] OIDC-only authentication accepted (no AWS keys in GitHub).
- [ ] Plan-first, apply-later rollout accepted.
- [ ] GitHub approval capability checked; fallback accepted if needed.
- [ ] Bootstrap OIDC role changes approved for local apply.
- [ ] No PR plans initially.
- [ ] KMS CMK deferred; SSE-S3 accepted for state.
- [ ] Bootstrap state migration to S3 approved.

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
