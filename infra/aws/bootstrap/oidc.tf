variable "github_owner" {
  description = "GitHub repository owner."
  type        = string
  default     = "amagarzogonzalo"
}

variable "github_owner_id" {
  description = "Immutable GitHub owner ID used in the OIDC subject."
  type        = number
  default     = 132710511
}

variable "github_repository" {
  description = "GitHub repository name."
  type        = string
  default     = "eval-service"
}

variable "github_repository_id" {
  description = "Immutable GitHub repository ID used in the OIDC subject."
  type        = number
  default     = 1426901614
}

locals {
  github_main_subject = "repo:${var.github_owner}@${var.github_owner_id}/${var.github_repository}@${var.github_repository_id}:ref:refs/heads/main"
}

data "tls_certificate" "github" {
  url = "https://token.actions.githubusercontent.com"
}

resource "aws_iam_openid_connect_provider" "github" {
  url            = "https://token.actions.githubusercontent.com"
  client_id_list = ["sts.amazonaws.com"]
  thumbprint_list = distinct(concat(
    [data.tls_certificate.github.certificates[length(data.tls_certificate.github.certificates) - 1].sha1_fingerprint],
    ["6938fd4d98bab03faadb97b34396831e3780aea1"],
  ))
}

data "aws_iam_policy_document" "github_plan_assume" {
  statement {
    sid     = "GitHubMainRefOnly"
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [aws_iam_openid_connect_provider.github.arn]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = [local.github_main_subject]
    }
  }
}

resource "aws_iam_role" "github_plan" {
  name                 = "eval-github-plan"
  description          = "Read-only Terraform plan role for GitHub Actions on main."
  assume_role_policy   = data.aws_iam_policy_document.github_plan_assume.json
  max_session_duration = 3600
}

resource "aws_iam_role_policy_attachment" "github_plan_readonly" {
  role       = aws_iam_role.github_plan.name
  policy_arn = "arn:aws:iam::aws:policy/ReadOnlyAccess"
}

data "aws_iam_policy_document" "github_plan_state" {
  statement {
    sid       = "ReadStateBucket"
    effect    = "Allow"
    actions   = ["s3:GetBucketLocation", "s3:ListBucket"]
    resources = [aws_s3_bucket.state.arn]
  }

  statement {
    sid       = "ReadStateObjects"
    effect    = "Allow"
    actions   = ["s3:GetObject"]
    resources = ["${aws_s3_bucket.state.arn}/eval-runner/*"]
  }

  statement {
    sid       = "LockState"
    effect    = "Allow"
    actions   = ["s3:PutObject", "s3:DeleteObject"]
    resources = ["${aws_s3_bucket.state.arn}/eval-runner/terraform.tfstate.tflock"]
  }
}

resource "aws_iam_role_policy" "github_plan_state" {
  name   = "github-plan-state"
  role   = aws_iam_role.github_plan.id
  policy = data.aws_iam_policy_document.github_plan_state.json
}

output "github_oidc_provider_arn" {
  value = aws_iam_openid_connect_provider.github.arn
}

output "github_plan_role_arn" {
  value = aws_iam_role.github_plan.arn
}
