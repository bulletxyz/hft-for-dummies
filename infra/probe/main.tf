# One tiny instance per availability zone. Each measures its round trip to
# Binance and Bullet at boot and prints it to its console log. ./run.sh
# collects the numbers and destroys everything.
#
# Zones are keyed by zone ID (apne1-az1), not name (ap-northeast-1a): names
# are shuffled per AWS account, IDs are the same building for everyone.

provider "aws" {
  region = "ap-northeast-1" # Tokyo: where Binance and Bullet run
}

# Your account's default VPC has one subnet per zone.
data "aws_subnets" "default" {
  filter {
    name   = "default-for-az"
    values = ["true"]
  }
}

data "aws_subnet" "zone" {
  for_each = toset(data.aws_subnets.default.ids)
  id       = each.key
}

data "aws_ssm_parameter" "al2023" {
  name = "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64"
}

resource "aws_instance" "probe" {
  for_each      = { for s in data.aws_subnet.zone : s.availability_zone_id => s.id }
  ami           = data.aws_ssm_parameter.al2023.value
  instance_type = "t4g.small"
  subnet_id     = each.value
  user_data     = file("${path.module}/probe.sh")
  tags          = { Name = "hft-probe-${each.key}" }
}

output "instances" {
  value = { for zone, i in aws_instance.probe : zone => i.id }
}
