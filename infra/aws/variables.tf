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
  description = "EC2 instance type. t4g.small is free for 750 h/month until 2026-12-31."
  type        = string
  default     = "t4g.small"
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
  description = "Monthly budget alarm limit. The runner itself is free; this catches mistakes."
  type        = string
  default     = "1"
}

variable "boot_volume_gb" {
  description = "Root EBS size in GiB. Keep small; legacy free tier allows 30 GiB."
  type        = number
  default     = 30
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
