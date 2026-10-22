variable "region" {
  description = "AWS region. eu-north-1 is in the T4g free-trial region list."
  type        = string
  default     = "eu-north-1"
}

variable "project" {
  description = "Name prefix for resources."
  type        = string
  default     = "eval-runner"
}

variable "instance_type" {
  description = "EC2 instance type. Current production uses t4g.large; t4g.small is the low-cost option."
  type        = string
  default     = "t4g.large"

  validation {
    condition     = startswith(var.instance_type, "t4g.")
    error_message = "Only Graviton t4g.* shapes keep the ARM64/free-trial posture."
  }
}

variable "protect_instance" {
  description = "Block EC2 API termination. Set false only when intentionally destroying."
  type        = bool
  default     = true
}

variable "owner" {
  description = "Owner tag used by cost reporting."
  type        = string
  default     = "amagarzogonzalo"
}

variable "admin_cidr" {
  description = "CIDR allowed to SSH (your public IP /32). Never use 0.0.0.0/0."
  type        = string

  validation {
    condition     = can(cidrhost(var.admin_cidr, 0)) && var.admin_cidr != "0.0.0.0/0"
    error_message = "admin_cidr must be a valid non-default CIDR such as 203.0.113.7/32."
  }
}

variable "ssh_public_key_path" {
  description = "Path to the SSH public key installed for ubuntu/eval."
  type        = string
  default     = "~/.ssh/eval-runner-aws.pub"
}

variable "alert_email" {
  description = "Email address for AWS Budget alerts (must be confirmed once)."
  type        = string
}

variable "budget_usd" {
  description = "Monthly budget limit. Budgets are alerts + an automatic stop action, not a hard cap."
  type        = string
  default     = "60"

  validation {
    condition     = can(tonumber(var.budget_usd)) && tonumber(var.budget_usd) > 0
    error_message = "budget_usd must be a positive number."
  }
}

variable "boot_volume_gb" {
  description = "Root EBS size in GiB. Keep small; legacy free tier allows 30 GiB."
  type        = number
  default     = 30

  validation {
    condition     = var.boot_volume_gb >= 20 && var.boot_volume_gb <= 30
    error_message = "Keep the boot volume between 20 and 30 GiB to bound monthly storage cost."
  }
}

variable "store_bucket_name" {
  description = "Optional globally unique S3 bucket name for the eval store. Computed when empty."
  type        = string
  default     = ""
}

variable "enable_ssm" {
  description = "Install the SSM agent and grant the instance SSM core permissions."
  type        = bool
  default     = true
}

