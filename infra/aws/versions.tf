terraform {
  # use_lockfile below needs Terraform 1.10+.
  required_version = ">= 1.10.0"

  # Remote state, created by infra/aws/bootstrap. Native S3 locking
  # (Terraform >= 1.10) replaces DynamoDB. Backends cannot use variables,
  # so account/region are intentionally explicit for this single stack.
  backend "s3" {
    bucket       = "eval-tfstate-REPLACE_WITH_ACCOUNT_ID-eu-north-1"
    key          = "eval-runner/terraform.tfstate"
    region       = "eu-north-1"
    use_lockfile = true
    encrypt      = true
  }

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.60, < 7.0"
    }
  }
}

provider "aws" {
  region = var.region

  default_tags {
    tags = {
      Project   = "eval-service"
      Component = "runner"
      ManagedBy = "terraform"
      Owner     = var.owner
    }
  }
}
