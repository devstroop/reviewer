#### Schema Correctness
- Invalid YAML syntax — indentation errors, tabs vs spaces, missing colons
- Wrong value types — strings where numbers expected, arrays where objects expected
- Required keys missing for the target system
- Anchors (`&`) and aliases (`*`) incorrectly referenced or cyclical

#### Security-Sensitive Values
- `debug: true` or `log_level: debug` in production configs
- Permissive CORS origins (`Access-Control-Allow-Origin: "*"`)
- Authentication disabled (`auth: false`, `authentication: none`)
- Overly broad IAM roles or ACLs (`Action: "*"`, `effect: Allow` on all resources)
- Hardcoded secrets, tokens, or connection strings

#### Environment Drift
- Values that differ from documented defaults without explanation
- Staging or development values (e.g. internal hostnames, staging API URLs) leaking into production configs
- Environment names inconsistent with naming conventions

#### Resource Constraints
- Unreasonably low timeouts that would cause failures under normal load
- Connection pool sizes too small for expected concurrency
- Rate limits set too low or missing entirely
- Memory or disk limits that don't account for peak usage
