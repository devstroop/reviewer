# Contributing

We welcome contributions! Here's how to get started.

## Prerequisites
- Rust 1.85+ (MSRV)
- Docker (for building the Action image)

## Development

```bash
# Build
cargo build

# Run all tests
cargo test

# Run specific test
cargo test test_name

# Lint (must pass with no warnings)
cargo clippy -- -D warnings

# Check formatting
cargo fmt --check

# Generate documentation
cargo doc --no-deps --open

# Run a review locally
# Prerequisite: Set GITHUB_TOKEN and AI_API_KEY in your environment first.
cargo run -- review --pr-url https://github.com/owner/repo/pull/1

# Review a single file
cargo run -- review-file src/main.rs

# Review via stdin
cargo run -- review-stdin --title "My change" < diff.patch

# Review files matching a glob
cargo run -- review-glob "src/**/*.rs"

# Review a local git branch diff
cargo run -- review-local --repo-path . --base-ref main --head-ref feature

# Start MCP stdio server (for AI agent integration)
cargo run -- mcp

# Start webhook server
cargo run -- serve --port 8080
```

## Project Structure
```
src/
├── main.rs           # CLI (review, serve, mcp, review-stdin, review-file, review-local, review-glob)
├── lib.rs            # Module tree + re-exports
├── engine.rs         # Core ReviewEngine (standard, tool-loop, concurrent paths)
├── config.rs         # Settings (TOML + env overlay, Sensitive<T> secrets)
├── error.rs          # AgentError enum (thiserror)
├── sensitive.rs      # Sensitive<T> wrapper (redacts Display/Debug/Serialize)
├── session.rs        # JSONL session persistence (resume support)
├── tokens.rs         # Token estimation (tiktoken BPE, falls back to heuristic)
├── diff.rs           # Unified diff parser via diffy
├── language.rs       # Extension → language lookup
├── logging.rs        # tracing subscriber (env-filter, JSON)
├── sarif.rs          # SARIF output format
├── github/           # GitHub API client (reqwest, rate-limit, retry, find/edit comments)
│   ├── mod.rs
│   └── types.rs
├── git/              # Local git integration via git2
│   ├── mod.rs
│   └── local.rs
├── ai/               # OpenAI-compatible chat client (chat, chat_with_tools)
│   ├── mod.rs
│   └── types.rs
├── tools/            # LLM tool registry + tools (file_read, code_search, submit_finding, etc.)
│   ├── mod.rs
│   ├── registry.rs
│   ├── review.rs
│   ├── file_read.rs
│   ├── code_search.rs
│   ├── file_find.rs
│   ├── submit_finding.rs
│   └── task_done.rs
├── mcp/              # MCP server tools
│   └── tools.rs
├── services/         # Shared services (prompt_builder, diff_service, file_reader, etc.)
│   ├── mod.rs
│   ├── prompt_builder.rs
│   ├── github_service.rs
│   ├── diff_service.rs
│   ├── file_reader.rs
│   ├── json_extractor.rs
│   ├── review_filter.rs
│   ├── relocation.rs
│   └── sanitize.rs
└── server/           # Webhook server
    └── mod.rs
```

## Code Style
- Run `cargo fmt` before committing
- Clippy must pass with `-- -D warnings` (warnings are errors)
- Keep `default-features = false` on dependencies where possible to minimize binary size
- Add tests for new functionality — use `wiremock` for HTTP mocking (no network in CI)
- All public types and functions must have `///` doc comments

## Pull Request Process
1. Create a feature branch from `develop`
2. Ensure `cargo test` passes and `cargo clippy -- -D warnings` is clean
3. Ensure `cargo fmt --check` passes
4. Update documentation (README, AGENTS.md) for any new features
5. For config changes, update `reviewer.toml` and the README config reference
6. Open a PR against `develop` with a clear description of the change
