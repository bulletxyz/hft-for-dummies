# One server in Tokyo, in the zone ./infra/probe/run.sh found closest to the
# exchanges, ready to build and run the bot. ./deploy.sh does the rest.

provider "aws" {
  region = "ap-northeast-1"
}

variable "zone_id" {
  description = "Zone ID from the probe (IDs, unlike names, mean the same zone in every account)."
  type        = string
  default     = "apne1-az4"
}

variable "instance_type" {
  description = "See docs/parts/04-colocation.md for why."
  type        = string
  default     = "c8g.large"
}

variable "my_ip" {
  description = "Only this IP may SSH in (deploy.sh fills it in)."
  type        = string
}

data "aws_subnet" "zone" {
  availability_zone_id = var.zone_id
  default_for_az       = true
}

data "aws_ssm_parameter" "al2023" {
  name = "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64"
}

resource "aws_key_pair" "bot" {
  key_name   = "hft-for-dummies"
  public_key = file("${path.module}/.ssh/id_ed25519.pub")
}

resource "aws_security_group" "bot" {
  name   = "hft-for-dummies"
  vpc_id = data.aws_subnet.zone.vpc_id

  ingress {
    description = "SSH from you only"
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = ["${var.my_ip}/32"]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

# The trading key lives in SSM Parameter Store (deploy.sh uploads it,
# encrypted). The server may read that one parameter and nothing else.
data "aws_caller_identity" "me" {}

resource "aws_iam_role" "bot" {
  name = "hft-for-dummies"
  assume_role_policy = jsonencode({
    Version   = "2012-10-17"
    Statement = [{ Effect = "Allow", Action = "sts:AssumeRole", Principal = { Service = "ec2.amazonaws.com" } }]
  })
}

resource "aws_iam_role_policy" "read_key" {
  role = aws_iam_role.bot.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = "ssm:GetParameter"
      Resource = "arn:aws:ssm:ap-northeast-1:${data.aws_caller_identity.me.account_id}:parameter/hft-for-dummies/bot-key"
    }]
  })
}

resource "aws_iam_instance_profile" "bot" {
  name = "hft-for-dummies"
  role = aws_iam_role.bot.name
}

resource "aws_instance" "bot" {
  ami                    = data.aws_ssm_parameter.al2023.value
  instance_type          = var.instance_type
  subnet_id              = data.aws_subnet.zone.id
  vpc_security_group_ids = [aws_security_group.bot.id]
  key_name               = aws_key_pair.bot.key_name
  iam_instance_profile   = aws_iam_instance_profile.bot.name
  user_data              = file("${path.module}/cloud-init.yaml")
  tags                   = { Name = "hft-for-dummies" }

  # A new AMI or user_data would replace the server; do that on purpose only.
  lifecycle {
    ignore_changes = [ami, user_data]
  }
}

output "ip" {
  value = aws_instance.bot.public_ip
}
