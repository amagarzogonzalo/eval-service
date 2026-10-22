output "instance_id" {
  value = aws_instance.runner.id
}

output "public_ip" {
  description = "Stable Elastic IPv4 attached to the runner."
  value       = aws_eip.runner.public_ip
}

output "ssh_command" {
  value = "ssh -i ~/.ssh/eval-runner-aws ubuntu@${aws_eip.runner.public_ip}"
}

output "store_bucket" {
  value = aws_s3_bucket.store.bucket
}

output "region" {
  value = var.region
}

output "ami" {
  value = data.aws_ami.ubuntu.id
}

output "ssm_session" {
  value = "aws ssm start-session --target ${aws_instance.runner.id} --region ${var.region}"
}

