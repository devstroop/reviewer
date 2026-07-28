#### Terraform-Specific
- Hardcoded resource identifiers — use variables or data sources instead
- Missing `lifecycle` block — `prevent_destroy` on critical resources, `ignore_changes` on mutable metadata
- State lock configuration missing or incorrectly configured
- Provider version not pinned — use `required_providers` with version constraint

#### Security
- Security group rules that allow `0.0.0.0/0` ingress on non-HTTP ports
- S3 bucket ACLs or policies that allow public access
- IAM policies with `Effect: "Allow"` and `Resource: "*"` without specific action constraints
- Encryption disabled on storage resources (S3, EBS, RDS)
- Secrets or sensitive values in plaintext — use `sensitive = true` or a secrets manager

#### Resource Management
- Resources without tags — add tagging conventions for cost allocation
- Hardcoded size/capacity values — make them configurable via variables
- Missing `depends_on` where implicit dependency ordering is insufficient
- Count/for_each used where a simpler approach would work

#### State & Backend
- Remote state without state locking (e.g. S3 without DynamoDB)
- State files committed to version control
- Workspace configuration missing for multi-environment setups
