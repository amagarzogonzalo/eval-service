data "aws_caller_identity" "current" {}

data "aws_availability_zones" "available" {
  state = "available"
}

data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical

  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-arm64-server-*"]
  }
  filter {
    name   = "architecture"
    values = ["arm64"]
  }
  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

locals {
  default_bucket = "eval-store-${data.aws_caller_identity.current.account_id}-${var.region}"
  bucket_name    = var.store_bucket_name != "" ? var.store_bucket_name : local.default_bucket
  ssh_public_key = trimspace(file(pathexpand(var.ssh_public_key_path)))
}

# --- network ---------------------------------------------------------------

resource "aws_vpc" "main" {
  cidr_block           = "10.42.0.0/16"
  enable_dns_support   = true
  enable_dns_hostnames = true

  tags = { Name = "${var.project}-vpc" }
}

resource "aws_internet_gateway" "main" {
  vpc_id = aws_vpc.main.id

  tags = { Name = "${var.project}-igw" }
}

resource "aws_subnet" "public" {
  vpc_id                  = aws_vpc.main.id
  cidr_block              = "10.42.1.0/24"
  availability_zone       = data.aws_availability_zones.available.names[0]
  map_public_ip_on_launch = false

  tags = { Name = "${var.project}-public" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.main.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.main.id
  }

  tags = { Name = "${var.project}-public" }
}

resource "aws_route_table_association" "public" {
  subnet_id      = aws_subnet.public.id
  route_table_id = aws_route_table.public.id
}

resource "aws_security_group" "runner" {
  name        = "${var.project}-sg"
  description = "SSH from the administrator only; unrestricted egress"
  vpc_id      = aws_vpc.main.id

  ingress {
    description = "SSH from admin"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = [var.admin_cidr]
  }

  egress {
    description = "All outbound (providers, GitHub, OCI/AWS APIs)"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = { Name = "${var.project}-sg" }
}

# --- storage ---------------------------------------------------------------

resource "aws_s3_bucket" "store" {
  bucket = local.bucket_name

  # Never let `terraform destroy` silently delete the evidence store.
  force_destroy = false

  tags = { Name = "${var.project}-store" }
}

resource "aws_s3_bucket_versioning" "store" {
  bucket = aws_s3_bucket.store.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "store" {
  bucket = aws_s3_bucket.store.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "store" {
  bucket                  = aws_s3_bucket.store.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "store" {
  bucket = aws_s3_bucket.store.id

  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

data "aws_iam_policy_document" "store_tls" {
  statement {
    sid    = "DenyInsecureTransport"
    effect = "Deny"

    principals {
      type        = "*"
      identifiers = ["*"]
    }

    actions = ["s3:*"]

    resources = [
      aws_s3_bucket.store.arn,
      "${aws_s3_bucket.store.arn}/*",
    ]

    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "store" {
  bucket = aws_s3_bucket.store.id
  policy = data.aws_iam_policy_document.store_tls.json
}

resource "aws_s3_bucket_lifecycle_configuration" "store" {
  bucket = aws_s3_bucket.store.id

  rule {
    id     = "abort-incomplete-multipart"
    status = "Enabled"
    filter {}
    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# --- instance identity -----------------------------------------------------

data "aws_iam_policy_document" "assume_ec2" {
  statement {
    actions = ["sts:AssumeRole"]

    principals {
      type        = "Service"
      identifiers = ["ec2.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "runner" {
  name               = "${var.project}-role"
  assume_role_policy = data.aws_iam_policy_document.assume_ec2.json

  tags = { Name = "${var.project}-role" }
}

resource "aws_iam_role_policy_attachment" "ssm" {
  count      = var.enable_ssm ? 1 : 0
  role       = aws_iam_role.runner.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

data "aws_iam_policy_document" "runner" {
  statement {
    sid       = "StoreBucket"
    actions   = ["s3:ListBucket", "s3:GetBucketLocation"]
    resources = [aws_s3_bucket.store.arn]
  }

  statement {
    sid       = "StoreObjects"
    actions   = ["s3:GetObject", "s3:PutObject"]
    resources = ["${aws_s3_bucket.store.arn}/*"]
  }

  statement {
    sid     = "EvalParameters"
    actions = ["ssm:GetParameter", "ssm:GetParameters"]
    resources = [
      "arn:aws:ssm:${var.region}:${data.aws_caller_identity.current.account_id}:parameter/eval/*"
    ]
  }

  statement {
    sid       = "DecryptSecureStringsViaSsm"
    actions   = ["kms:Decrypt"]
    resources = ["*"]

    condition {
      test     = "StringEquals"
      variable = "kms:ViaService"
      values   = ["ssm.${var.region}.amazonaws.com"]
    }
  }
}

resource "aws_iam_role_policy" "runner" {
  name   = "${var.project}-policy"
  role   = aws_iam_role.runner.id
  policy = data.aws_iam_policy_document.runner.json
}

resource "aws_iam_instance_profile" "runner" {
  name = "${var.project}-profile"
  role = aws_iam_role.runner.name
}

# --- compute ---------------------------------------------------------------

resource "aws_key_pair" "runner" {
  key_name   = "${var.project}-key"
  public_key = local.ssh_public_key

  tags = { Name = "${var.project}-key" }
}

resource "aws_instance" "runner" {
  ami                         = data.aws_ami.ubuntu.id
  instance_type               = var.instance_type
  key_name                    = aws_key_pair.runner.key_name
  subnet_id                   = aws_subnet.public.id
  vpc_security_group_ids      = [aws_security_group.runner.id]
  iam_instance_profile        = aws_iam_instance_profile.runner.name
  associate_public_ip_address = true
  monitoring                  = false
  disable_api_termination     = var.protect_instance

  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required" # IMDSv2 only
    http_put_response_hop_limit = 1
  }

  # T4g defaults to unlimited CPU credits (surplus credits are billed).
  # Standard keeps the free trial strictly about the hourly allowance.
  credit_specification {
    cpu_credits = "standard"
  }

  root_block_device {
    volume_size           = var.boot_volume_gb
    volume_type           = "gp3"
    encrypted             = true
    delete_on_termination = true
  }

  user_data = templatefile("${path.module}/cloud-init.yaml", {
    project        = var.project
    region         = var.region
    bucket         = aws_s3_bucket.store.bucket
    ssh_public_key = local.ssh_public_key
  })

  # Bootstrap changes are applied deliberately with -replace, never as a
  # surprise instance rebuild during a routine apply.
  user_data_replace_on_change = false

  tags = { Name = "${var.project}-vm" }

  depends_on = [
    aws_iam_role_policy.runner,
    aws_iam_role_policy_attachment.ssm,
    aws_route_table_association.public,
  ]
}

# --- cost guardrail --------------------------------------------------------

resource "aws_budgets_budget" "monthly" {
  name         = "${var.project}-monthly"
  budget_type  = "COST"
  limit_amount = var.budget_usd
  limit_unit   = "USD"
  time_unit    = "MONTHLY"

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = "50"
    threshold_type             = "PERCENTAGE"
    notification_type          = "ACTUAL"
    subscriber_email_addresses = [var.alert_email]
  }

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = "80"
    threshold_type             = "PERCENTAGE"
    notification_type          = "ACTUAL"
    subscriber_email_addresses = [var.alert_email]
  }

  notification {
    comparison_operator        = "GREATER_THAN"
    threshold                  = "100"
    threshold_type             = "PERCENTAGE"
    notification_type          = "FORECASTED"
    subscriber_email_addresses = [var.alert_email]
  }
}

# Budgets are alerts, not hard caps. This action is the closest thing to a
# cap: at 100% of the monthly budget, AWS stops the runner via SSM. The
# fixed floor (public IPv4 + EBS) is why the default budget is $10, not $1.
data "aws_iam_policy_document" "budget_assume" {
  statement {
    actions = ["sts:AssumeRole"]

    principals {
      type        = "Service"
      identifiers = ["budgets.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "budget_action" {
  name               = "${var.project}-budget-action"
  assume_role_policy = data.aws_iam_policy_document.budget_assume.json

  tags = { Name = "${var.project}-budget-action" }
}

resource "aws_iam_role_policy_attachment" "budget_action" {
  role       = aws_iam_role.budget_action.name
  policy_arn = "arn:aws:iam::aws:policy/AWSBudgetsActions_RolePolicyForResourceAdministrationWithSSM"
}

resource "aws_budgets_budget_action" "stop_runner" {
  budget_name        = aws_budgets_budget.monthly.name
  action_type        = "RUN_SSM_DOCUMENTS"
  approval_model     = "AUTOMATIC"
  notification_type  = "ACTUAL"
  execution_role_arn = aws_iam_role.budget_action.arn

  action_threshold {
    action_threshold_type  = "PERCENTAGE"
    action_threshold_value = 100
  }

  definition {
    ssm_action_definition {
      action_sub_type = "STOP_EC2_INSTANCES"
      instance_ids    = [aws_instance.runner.id]
      region          = var.region
    }
  }

  subscriber {
    address           = var.alert_email
    subscription_type = "EMAIL"
  }

  depends_on = [aws_iam_role_policy_attachment.budget_action]

  tags = { Name = "${var.project}-stop-at-budget" }
}

# --- operational alarms ----------------------------------------------------

resource "aws_sns_topic" "alerts" {
  name = "${var.project}-alerts"
}

resource "aws_sns_topic_subscription" "email" {
  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = var.alert_email
}

resource "aws_cloudwatch_metric_alarm" "status_check" {
  alarm_name          = "${var.project}-status-check"
  alarm_description   = "EC2 status checks are failing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "StatusCheckFailed"
  namespace           = "AWS/EC2"
  period              = 60
  statistic           = "Maximum"
  threshold           = 0
  treat_missing_data  = "breaching"

  dimensions    = { InstanceId = aws_instance.runner.id }
  alarm_actions = [aws_sns_topic.alerts.arn]
}

resource "aws_cloudwatch_metric_alarm" "cpu_credits" {
  alarm_name          = "${var.project}-low-cpu-credits"
  alarm_description   = "T4g standard CPU credits are low; expect throttling"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 3
  metric_name         = "CPUCreditBalance"
  namespace           = "AWS/EC2"
  period              = 300
  statistic           = "Average"
  threshold           = 20
  treat_missing_data  = "notBreaching"

  dimensions    = { InstanceId = aws_instance.runner.id }
  alarm_actions = [aws_sns_topic.alerts.arn]
}
