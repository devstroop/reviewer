# AGENTS.md — reviewer

Single-binary Rust CLI that reviews GitHub PRs via any OpenAI-compatible endpoint. Runs as a Docker-based GitHub Action on `gcr.io/distroless/static` (~20 MB image).

## Architecture

```
src/
├── main.rs           — clap CLI (review, serve, mcp, review-stdin, review-file, review-local, review-glob)
├── lib.rs            — Module tree + re-exports
├── engine.rs         — Core ReviewEngine: ReviewRequest → ReviewResult (standard, tool-loop, concurrent paths)
├── config.rs         — Settings::load() — TOML + env overlay (Sensitive<T> for secrets)
├── error.rs          — AgentError enum (thiserror)
├── sensitive.rs      — Sensitive<T> — Display/Debug redacted as "***"
├── session.rs        — JSONL session files: start → file_done/file_skipped → end (resume support)
├── tokens.rs         — tiktoken-rs BPE token counting (falls back to heuristic)
├── diff.rs           — Diff parser via diffy crate; binary files auto-skipped
├── language.rs       — Extension → language lookup via sorted slice + Path::extension()
├── logging.rs        — tracing subscriber (env-filter, JSON format)
├── github/           — GitHub API client (reqwest, rate-limit, retry, find/edit comments)
│   ├── mod.rs
│   └── types.rs
├── git/              — Local git integration (git2: open, diff_between, file_at, grep)
│   ├── mod.rs
│   └── local.rs
├── ai/               — OpenAI-compatible chat client; chat, chat_with_tools, chat_with_max_tokens
│   ├── mod.rs
│   └── types.rs
├── tools/            — LLM tool registry + individual tools
│   ├── mod.rs
│   ├── registry.rs   — Tool trait + ToolRegistry (code_domain preset)
│   ├── file_read.rs  — Read files with canonicalize-based path traversal protection
│   ├── code_search.rs— git grep search
│   ├── file_find.rs  — Find files by name glob
│   ├── submit_finding.rs — Submit structured review findings
│   ├── task_done.rs  — Signal review completion
│   └── review.rs     — ReviewTool (orchestrates PR review from CLI)
├── mcp/              — MCP server tools (review_pr, review_diff, review_file, review_glob)
│   └── tools.rs
├── services/         — Shared services
│   ├── mod.rs
│   ├── prompt_builder.rs — Domain-aware prompt construction + rule resolution
│   ├── github_service.rs — Wraps GitHub API calls
│   ├── diff_service.rs   — Diff parsing + token budgeting
│   ├── file_reader.rs    — Read single file or glob, truncate content to budget
│   ├── json_extractor.rs — Extract structured JSON findings from AI output
│   ├── review_filter.rs  — Post-hoc false-positive detection
│   ├── relocation.rs     — Line-number correction via hunk matching + AI fallback
│   └── sanitize.rs       — Strip workflow commands, ANSI from AI output
├── sarif.rs          — SARIF output format for CI integration
└── server/           — Webhook server for real-time PR processing
    └── mod.rs
prompts/
├── code/system.txt   — Code review system prompt (with {system_rule}, {tool_loop_instructions} placeholders)
├── code/rules/       — Built-in rules: rules.json + individual .md files
├── config/           — Config review domain
├── compliance/       — Compliance review domain
├── data/             — Data review domain
└── policy/           — Policy review domain
Dockerfile            — Multi-stage Docker build (musl static → distroless/static)
action.yml            — GitHub Action metadata (Docker strategy)
```

## Conventions

- **Errors**: `AgentError` enum with `?` propagation. Add variants as needed.
- **Secrets**: `Sensitive<T>` wrapper — Display/Debug show `"***"`. All keys/tokens use it.
- **Config**: `$GITHUB_WORKSPACE/.github/reviewer.toml` → CWD → `~/.config/` → defaults. Env vars override.
- **Event filtering**: Action no-ops unless event is `opened`, `synchronize`, or `reopened`. Draft PRs and bot senders are skipped (ADR-019).
- **Diff source**: Fetched via `Accept: application/vnd.github.v3.diff` header, guaranteeing standard unified diff format (ADR-005).
- **GitHub rate limiting**: Semaphore(10) + `governor` (100 req/min). AI retry via `backoff` (exp+jitter, 3 retries, 429/5xx only).
- **Token budget**: tiktoken-rs BPE encoding (`cl100k_base`) for accurate token counting. Falls back to (len×2)/7 heuristic if encoding unavailable (ADR-006).
- **Review domains**: Each domain (code, config, compliance, policy, data) has its own `prompts/<domain>/system.txt` and `prompts/<domain>/user.txt`. The domain is resolved from the source.
- **Rule system**: `prompts/code/rules/rules.json` contains built-in rules. Project-level `.reviewer/rules.json` is merged on top. Rules are matched against changed file paths and injected via `{system_rule}` placeholder in system prompt.
- **Tool loop**: When `use_tools=true`, the engine short-circuits to `run_tool_loop` which gives the AI interactive tools (file_read, code_search, submit_finding). Results go through post-hoc filter + line relocation. GitHub posting respects the `sticky` flag.
- **Concurrent reviews**: `review_concurrent` spawns per-file AI calls via `tokio::spawn`, throttled by `Arc<Semaphore>(4)`. Accepts `rules_text` and `sticky` for feature parity with the standard path.
- **Session tracking**: `Session::new()` creates a JSONL file at `.reviewer/sessions/<id>.jsonl`. `record_file_done`/`finalize` write audit records. Session IDs are validated (alphanumeric + dash + underscore) for path traversal safety.
- **Path traversal**: The `file_read` tool uses `canonicalize()` to resolve the requested path relative to CWD and rejects anything outside it. Imports from `tools/file_read.rs`.
- **Engine flow**: `review()` in `engine.rs` resolves the source → parses diff → resolves rules → builds prompts → runs AI (standard / tool-loop / concurrent) → post-processes → posts to GitHub → finalizes session.
- **HTTP**: single `reqwest::Client` with rustls-tls. Headers: `User-Agent: reviewer`, `Accept: application/vnd.github.v3.diff`.
- **Logging**: `tracing` — JSON when `LOG_FORMAT=json`. Secrets redacted at type level.
- **Tests**: `wiremock` for HTTP mocking. No network in CI.
- **Docker**: Static link via `x86_64-unknown-linux-musl` target, `gcr.io/distroless/static` base image (ADR-018).
- **Action**: `action.yml` with Docker strategy — auto-detects PR URL from `github.event.pull_request`.
- **Release**: GHCR publish + GitHub Release on `v*` tags, SBOM generation.
- **Observability**: Step summary table via `$GITHUB_STEP_SUMMARY` — PR size, tokens, latency, model (ADR-020).
- **MSRV**: 1.85. Edition 2024.
- **Style**: `cargo fmt`, `cargo clippy` clean, `default-features=false` on deps.

## Key Docs

| File | What it's for |
|---|---|
| `DECISIONS.md` | All architecture decisions (why Rust, why no YAML, why Sensitive<T>, static linking, event filtering, step summary, etc.) |
| `SECURITY.md` | Threat model: prompt injection, secret leakage, token scopes |

## Current State

All phases complete. **219 tests pass**, 0 fail, 0 warnings (`cargo clippy -D warnings` clean).

| Phase | What | Status |
|-------|------|--------|
| 1–8 | Foundation → Documentation & Polish | ✅ `master` |
| 9 | Accurate token counting (tiktoken-rs BPE) | ✅ `develop` |
| 10 | LLM tool loop (file_read, code_search, file_find, submit_finding, task_done) | ✅ `develop` |
| 11 | Tool registry + AI client tool call support (ToolDef, ToolCall, ToolResult) | ✅ `develop` |
| 12 | Post-hoc accuracy filter + line-number relocation | ✅ `develop` |
| 13 | Sticky PR comments (find_comment, edit_comment, post_or_update_review) | ✅ `develop` |
| 14 | Local git review via git2 (LocalRepo, ReviewSource::LocalBranch) | ✅ `develop` |
| 15 | Rule system (built-in + project .reviewer/rules.json, 2000-token budget) | ✅ `develop` |
| 16 | New review domains (config, compliance, policy, data) | ✅ `develop` |
| 17 | Session persistence (JSONL files, resume support, session_id in result) | ✅ `develop` |
| 18 | MCP server integration (review_pr, review_diff, review_file, review_glob) | ✅ `develop` |
| 19 | Concurrent per-file review (Arc<Semaphore>, review_concurrent) | ✅ `develop` |
| 20 | SARIF output format | ✅ `develop` |
| 21 | Webhook server for real-time processing | ✅ `develop` |
| 22 | Path traversal protection (canonicalize-based in file_read tool) | ✅ `develop` |
| 23 | Session security (ID validation, SHA-256 fingerprints) | ✅ `develop` |
