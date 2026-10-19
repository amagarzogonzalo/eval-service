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
    actions   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
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
