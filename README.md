# Reviewer

AI-powered reviewer — a single-binary Rust CLI that reviews GitHub Pull Requests using any OpenAI-compatible AI endpoint. Deployable as a Docker-based GitHub Action.

**License:** MIT | **MSRV:** 1.85 | **Base image:** `gcr.io/distroless/static` (~20 MB)

---

## Quickstart

### As a GitHub Action

```yaml
# .github/workflows/reviewer.yml
name: Reviewer
on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]
jobs:
  review:
    if: ${{ github.event.sender.type != 'Bot' && !github.event.pull_request.draft }}
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - name: Run reviewer
        uses: devstroop/reviewer@v0.1.0
        with:
          pr_url: ${{ format('https://github.com/{0}/{1}/pull/{2}', github.repository_owner, github.event.repository.name, github.event.pull_request.number) }}
          ai_api_key: ${{ secrets.AI_API_KEY }}
        env:
          AI_API_BASE: ${{ secrets.AI_API_BASE || 'https://ai.cloudmagic.io/v1' }}
          MODEL: ${{ secrets.MODEL || 'glm-4.6' }}
```

Only fires on `opened`, `synchronize`, and `reopened` events. Draft PRs and bot senders are skipped by default. `GITHUB_TOKEN` defaults to the auto-generated token.

### Local CLI

```bash
export GITHUB_TOKEN=ghp_...
export AI_API_KEY=sk-...

# Review a PR
cargo run -- review --pr-url https://github.com/owner/repo/pull/1

# Review via stdin
cargo run -- review-stdin --title "My change" < diff.patch

# Review a single file
cargo run -- review-file src/main.rs

# Review files matching a glob
cargo run -- review-glob "src/**/*.rs"

# Review a local git branch diff
cargo run -- review-local --repo-path . --base-ref main --head-ref feature

# Start an MCP stdio server (for AI agent integration)
cargo run -- mcp

# Start a webhook server (for real-time processing)
cargo run -- serve --port 8080
```

See [Configuration](#configuration) for setting up `reviewer.toml`.

---

## Features

| Feature | Description |
|---|---|
| **PR review** | Fetches diff from GitHub, analyzes with AI, posts structured review |
| **Sticky comments** | Edits existing review comments on re-review instead of posting new ones |
| **Interactive tool loop** | AI can read files, search code, find files, and submit findings autonomously |
| **Concurrent file review** | Multi-file sources reviewed in parallel with configurable concurrency |
| **Structured JSON findings** | AI outputs severity, category, file, line, and suggestions in JSON |
| **Post-hoc accuracy filtering** | AI re-evaluates findings to remove false positives |
| **Line-number relocation** | Maps findings to correct PR line numbers via hunk matching |
| **Session persistence** | JSONL session files for resume support and audit trails |
| **Local git review** | Review diffs between any two refs in a local repository |
| **File / glob / stdin** | Review sources without a GitHub PR |
| **Multiple domains** | Code, config, compliance, policy, and data review prompts |
| **Rule system** | Project-specific rules loaded from `.reviewer/rules.json` + built-in rules |
| **MCP server** | Model Context Protocol stdio server for AI agent integration |
| **Webhook server** | HTTP server for real-time PR review processing |
| **SARIF output** | Static Analysis Results Interchange Format for CI integration |
| **Accurate token counting** | BPE tokenization via tiktoken-rs (falls back to heuristic) |

---

## Configuration

Config is loaded from the first file found in this order:
1. `$GITHUB_WORKSPACE/.github/reviewer.toml` (GitHub Action)
2. `$CWD/reviewer.toml`
3. `$CWD/.reviewer.toml`
4. `~/.config/reviewer/config.toml`
5. Built-in defaults

Environment variables override file values.

### Reference

```toml
[ai]
api_base = "https://ai.cloudmagic.io/v1"   # OpenAI-compatible endpoint
model = "glm-4.6"                           # Model name
# api_key — set via AI_API_KEY env var
request_timeout_secs = 120                   # AI request timeout
temperature = 0.2                            # Model temperature (0.0–1.0)
max_completion_tokens = 4096                 # Max tokens in AI response

[github]
# token — set via GITHUB_TOKEN env var
request_timeout_secs = 30                    # GitHub API timeout
max_concurrent_requests = 10                 # Max concurrent API requests

[review]
max_input_tokens = 16000                     # Max tokens for diff input (tiktoken-counted)
max_diff_files = 50                          # Max files to review
extra_instructions = ""                      # Extra prompt instructions
```

### Required Env Vars

| Variable | Description |
|---|---|
| `GITHUB_TOKEN` | GitHub token (`contents:read` + `pull-requests:write`) |
| `AI_API_KEY` | API key for the AI endpoint |

### Optional Env Vars

| Variable | Overrides config field |
|---|---|
| `AI_API_BASE` | `ai.api_base` |
| `MODEL` | `ai.model` |
| `LOG_FORMAT` | Set to `"json"` for structured JSON logging |

---

## GitHub Token Guide

reviewer uses the GitHub token for:
1. **Reading PR diffs and metadata** — requires `contents: read`
2. **Posting reviews and comments** — requires `pull-requests: write`

### Default GITHUB_TOKEN (recommended for Actions)

```yaml
permissions:
  contents: read
  pull-requests: write
```

### Personal Access Token (for CLI use)

Create a [fine-grained PAT](https://github.com/settings/tokens?type=beta) with:
- Repository access: the repos you want to review
- Permissions: `contents: read`, `pull-requests: write`

**Do not use a token with the `repo` scope** — it grants unnecessary broad access.

---

## Architecture

The `review` command runs through a modular processing pipeline:

```
┌─────────────────────────┐
│  Source Resolution      │  GitHub PR, diff text, stdin, file, glob, local branch
├─────────────────────────┤
│  Config + Rules Load    │  TOML + env overlay; project/built-in rules merged
├─────────────────────────┤
│  Diff Fetch & Parse     │  GitHub API / git2 / file read → diffy parser → filtered files
├─────────────────────────┤
│  Token Budget           │  tiktoken-rs BPE counting; largest files dropped first
├─────────────────────────┤
│  AI Review              │  System prompt (domain-specific) + user prompt → AI
│  ├─ Standard path       │  Single-shot with JSON extraction + repair + filter
│  ├─ Tool loop path      │  Interactive: file_read/code_search → submit_finding → task_done
│  └─ Concurrent path     │  Parallel per-file reviews (multi-file sources)
├─────────────────────────┤
│  Post-processing        │  False-positive filtering + line-number relocation
├─────────────────────────┤
│  Output                 │  GitHub comment (optionally sticky) / stdout / SARIF
└─────────────────────────┘
```

### Skip Logic

Files are skipped when:
- The file matches the skip-list: `Cargo.lock`, `package-lock.json`, `yarn.lock`, `*.min.js`, `*.min.css`, `*.pb.go`, `*.pb.rs`, `CHANGELOG.md`, `vendor/`, `node_modules/`
- The file is binary (no `patch` data)
- The diff exceeds `max_diff_files` (default 50)
- The token budget exceeds `max_input_tokens` (default 16,000)
- The file path doesn't match the caller-supplied `paths` filter

### Review Domains

| Domain | Prompt | Focus |
|---|---|---|
| `code` | `prompts/code/system.txt` | Logic errors, security, performance, API misuse |
| `config` | `prompts/config/system.txt` | Infrastructure, deployment, secrets management |
| `compliance` | `prompts/compliance/system.txt` | Regulatory frameworks, audit trails, access controls |
| `policy` | `prompts/policy/system.txt` | Organisational policies, naming, licensing |
| `data` | `prompts/data/system.txt` | Schema changes, data quality, PII exposure |

### Rule System

Place rules in `.reviewer/rules.json` (project-level) or use built-in rules:

```json
{
  "rules": [
    {
      "pattern": "src/**/*.rs",
      "rule": "All public APIs must have doc comments"
    }
  ]
}
```

Rules are matched against changed files and injected into the AI's system prompt, up to a 2000-token budget.

---

## Security

See `SECURITY.md` for full details. Key points:
- **Secrets never logged**: `Sensitive<T>` wrapper redacts all keys/tokens in Display/Debug
- **Prompt injection**: AI output capped at 4096 tokens; workflow commands stripped; GITHUB_TOKEN excluded from AI context
- **Path traversal protection**: `file_read` tool canonicalises paths and rejects anything outside CWD
- **Token scope**: minimum `contents: read` + `pull-requests: write` — never use `repo` scope
- **Session ID validation**: alphanumeric, dash, underscore only — prevents path injection
- **Static linking**: Binary statically linked on `gcr.io/distroless/static` — no dynamic linker attack surface
- `tiktoken-rs` for accurate token counting (it's pure Rust now)

---

## Development

```bash
cargo build
cargo test
cargo clippy
cargo run -- review --pr-url https://github.com/owner/repo/pull/1
```

See `CONTRIBUTING.md` for detailed development guidelines.

---

## Comparison with PR-Agent

| Feature | reviewer | PR-Agent |
|---|---|---|
| Written in | Rust (single binary) | Python |
| Dependencies | ~15 crates, static-linked | 30+ Python packages |
| Image size | < 50 MB (target) | ~500 MB+ |
| Config | TOML + env vars | Dynaconf (TOML + env + chained loaders) |
| AI providers | Any OpenAI-compatible endpoint | 100+ via LiteLLM |
| Git hosts | GitHub only (v1) | GitHub, GitLab, Bitbucket, Azure, Gitea, Gerrit, etc. |
| Output | Raw markdown | Structured YAML + markdown |
| Tools | `review` only (v1) | `review`, `describe`, `improve`, `ask`, etc. |
| Inline suggestions | v2 | v1 |
| Incremental reviews | v2 | v1 |
