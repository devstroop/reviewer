#### Security (all languages)
- Hardcoded secrets, API keys, passwords, tokens, or connection strings
- SQL / NoSQL injection via unsanitised user input in queries
- Command injection — unsanitised input in shell commands, `os.system()`, `subprocess`, `exec`
- Path traversal — user input used in file paths without validation
- Unsafe deserialisation — `pickle`, `JSON.parse` on untrusted data, `eval()`
- Authentication or authorisation checks missing on protected endpoints

#### Dependency Security
- Packages fetched from untrusted sources or registries
- Pinned dependency versions that are known to be vulnerable
- Importing deprecated or removed packages
- Dynamic dependency loading from user-controlled paths

#### Data Handling
- Logging sensitive data (passwords, tokens, PII)
- Storing secrets in environment variables that are logged or exposed
- Insufficient input validation before processing
- Missing rate limiting on authentication endpoints
