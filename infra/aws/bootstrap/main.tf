# Bootstrap for the Terraform remote-state bucket. This stack keeps its
# own state locally (it is tiny and re-appliable); the main stack uses
# the bucket via the S3 backend with native lockfiles.
#
# The bucket name is deterministic from the account and region; pass the
# backend_bucket output to the main stack's init (Terraform backends cannot
# use variables).

variable "region" {
  type    = string
  default = "eu-north-1"
}

data "aws_caller_identity" "current" {}

locals {
  bucket = "eval-tfstate-${data.aws_caller_identity.current.account_id}-${var.region}"
}

resource "aws_s3_bucket" "state" {
  bucket        = local.bucket
  force_destroy = false

  tags = { Name = local.bucket }
}

resource "aws_s3_bucket_versioning" "state" {
  bucket = aws_s3_bucket.state.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "state" {
  bucket                  = aws_s3_bucket.state.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "state" {
  bucket = aws_s3_bucket.state.id

  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

data "aws_iam_policy_document" "state_tls" {
  statement {
    sid    = "DenyInsecureTransport"
    effect = "Deny"

    principals {
      type        = "*"
      identifiers = ["*"]
    }

    actions = ["s3:*"]

    resources = [
      aws_s3_bucket.state.arn,
      "${aws_s3_bucket.state.arn}/*",
    ]

    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "state" {
  bucket = aws_s3_bucket.state.id
  policy = data.aws_iam_policy_document.state_tls.json
}

output "backend_bucket" {
  value = aws_s3_bucket.state.bucket
}

output "backend_key" {
  value = "eval-runner/terraform.tfstate"
}
