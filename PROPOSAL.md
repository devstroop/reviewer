# reviewer — domain-agnostic AI review engine

A single-binary, domain-agnostic review engine. It accepts input from any
source (PR, file, glob, pipe, webhook, MCP), loads domain-specific review
prompts from `prompts/{domain}/`, sends them to any OpenAI-compatible AI, and
returns structured findings. Domains are plain files — adding one requires zero
code changes.

Neither a "code review tool" nor a "PR reviewer." Those are use cases. The
engine is the product. Code is one domain. Config is another. Policy,
security, design, data, compliance, legal — any domain where a reviewer with
the right prompts can examine content and produce findings.

---

## Why

Existing review tools are locked to a single domain and platform:

| Tool | Domain | Input | Output | MCP |
|---|---|---|---|---|
| PR-Agent | Code | PR only | Markdown | No |
| OpenCodeReview | Code | Diff/scan | JSON | No |
| GitLens | Code | PR (IDE) | Visual | No |
| **reviewer** | **Any** | **Any** | **Any** | **Yes** |

They are purpose-built for code review on GitHub. Our engine makes no such
assumption. The same pipeline that reviews a Rust PR can review a Terraform
plan, a Kubernetes manifest diff, a dependency policy change, or a legal
document revision — just by swapping prompt files.

---

## Architecture

```
                   ┌──────────────────────────────────────┐
                   │            IO Surfaces               │
                   │  CLI │ Action │ Webhook │ MCP        │
                   └────────┬─────────────────────────────┘
                            │
                            ▼
                   ┌──────────────────────────────────────┐
                   │        ReviewEngine                  │
                   │  ReviewRequest → ReviewResult        │
                   │                                      │
                   │  1. Resolve source (any input type)  │
                   │  2. Load prompts (any domain)        │
                   │  3. Parse + filter (domain-generic)  │
                   │  4. AI analysis                      │
                   │  5. Extract structured findings      │
                   │  6. Post (optional, any platform)    │
                   └──────────────────────────────────────┘
                            │
              ┌─────────────┼─────────────┐
              ▼             ▼             ▼
        Input sources   Prompt dirs    Output formats
     ┌──────────────┐  ┌───────────┐  ┌─────────────┐
     │ PrUrl        │  │ code/     │  │ reviewer    │
     │ DiffText     │  │ config/   │  │ JSON        │
     │ Stdin        │  │ policy/   │  │ SARIF       │
     │ File         │  │ compliance│  │ Markdown    │
     │ Glob         │  │ ...       │  │ (extensible)│
     │ LocalBranch  │  │ (add any) │  │             │
     └──────────────┘  └───────────┘  └─────────────┘
```

### Core contract

```rust
ReviewRequest {
    source: ReviewSource,   // what to review
    options: ReviewOptions, // how to review (paths, extras, post)
}

ReviewResult {
    review_text: String,       // AI's markdown analysis
    findings: Vec<Finding>,    // structured findings
    stats: ReviewStats,        // latency, tokens, counts
}
```

This single contract is shared by all six IO surfaces. Any surface can send
any domain to any AI and get structured results back.

### Domains are files

```
prompts/
├── code/
│   ├── system.txt    # "You are a code review assistant..."
│   └── user.txt      # Template with {diff}, {file_list}, etc.
├── config/
│   ├── system.txt    # "You are a configuration audit assistant..."
│   └── user.txt
├── policy/
│   ├── system.txt
│   └── user.txt
├── compliance/
│   ├── system.txt
│   └── user.txt
└── ...               # Drop a directory, get a new domain
```

The `PromptBuilder` resolves `prompts/{domain}/{system,user}.txt` at runtime
with a fallback chain: `{domain}` → `code` → compiled default. No code
changes. No recompilation. New domains ship as prompt files.

---

## Current state

### What exists (shippable)

| Layer | Status |
|---|---|
| CLI (6 subcommands) | ✅ |
| MCP server (6 tools) | ✅ |
| Webhook server | ✅ |
| ReviewEngine pipeline | ✅ |
| DiffService (parse/filter/budget) | ✅ |
| PromptBuilder (file-based domains) | ✅ |
| GitHub posting | ✅ |
| SARIF output | ✅ |
| JSON findings extraction + repair | ✅ |
| Sensitive\<T\> secret safety | ✅ |
| 190 tests | ✅ |
| Docker distroless image (~20MB) | ✅ |
| GitHub Action | ✅ |
| Release workflow | ✅ |
| PR #36 (multi-source, MCP, SARIF) | 🔶 unmerged (fmt fix pending) |

### What needs improvement

| Gap | Impact | Priority |
|---|---|---|
| No tool loop (single-shot AI call) | AI can't read files or search code | High |
| No rule/skills injection | Project-specific guidelines can't reach the AI | High |
| No review filter (FP detection) | False positives degrade trust | Medium |
| No re-location | Line numbers can be wrong | Medium |
| No per-repo config | Can't customize per project | Medium |
| GitHub API only for diff fetching | Rate-limited, no file content | Medium |
| No comment dedup/idempotency | Duplicate reviews on re-run | Low |

---

## Roadmap

### Phase 0 — Ship what we have
- Merge PR #36 (fix `cargo fmt`, release v0.2.0)
- Multi-source, MCP, SARIF, config domain go live

### Phase 1 — gix: local git for the code domain
The engine stays domain-agnostic. gix is a **code-domain-specific enhancer**.

- Add `src/git/` module wrapping `gix::Repository`
- New `ReviewSource::LocalBranch { repo_path, base_ref, head_ref }`
- Code domain uses gix for: local diff computation, full file content access,
  code search (`git grep`)
- GitHub API remains the fallback (CI ephemeral runners) and always used for posting
- Other domains (config, policy, etc.) are unaffected — they don't need git

**What this unlocks specifically for the code domain:**
- Full file content in prompts (not just 3-line diff context)
- `file_read` tool in the LLM tool loop
- `code_search` tool for repo-wide analysis
- Offline review capability
- Zero rate limits on read operations

### Phase 2 — LLM tool loop
The engine grows a tool-calling loop. Tools are **domain-specific devices**.

- Code domain tools: `file_read`, `code_search`, `file_find`, `task_done`
- Config/policy domains: `file_read` (no code_search — irrelevant)
- Single-shot remains the default for simple reviews
- Tool loop activates when the engine detects it would help (large diffs,
  cross-file dependencies) or when the AI requests it

### Phase 3 — Rule system (domain-generic)
Rules are plain markdown files matched by glob pattern — works for any
file-based domain. Single-layer (no complex priority resolver):

```
prompts/code/rules/
├── rust.md           # matches **/*.rs
├── python.md         # matches **/*.py
└── security.md       # matches **/*.rs, **/*.py  (cross-cutting)

prompts/config/rules/
├── kubernetes.md     # matches **/*.yaml
└── terraform.md      # matches **/*.tf
```

Injected via `{system_rule}` placeholder in system prompts.

### Phase 4 — Review filter + re-location (domain-generic)
Post-hoc passes that work for any domain with findings:

- **Review filter**: AI reviews its own findings and flags false positives
- **Re-location**: AI realigns mispositioned line numbers against the source

Both are optional, configurable per-domain.

### Phase 5 — Self-reflection scoring (code domain first)
- After generating findings, a reasoning model scores each (0-10)
- Low-confidence findings are either dropped or flagged
- PR-Agent's `/improve` pattern, adapted for any domain

### Phase 6 — Comment history & inline posting (platform-specific)
- Sticky comments that update across runs (idempotency)
- Line-anchored inline suggestions on diff hunks
- Platform-specific (GitHub first, GitLab later)

---

## Design invariants

1. **The engine is agnostic.** Every feature must work for any domain unless
   explicitly domain-specific. gix is code-only. The tool loop is generic but
   tool sets are per-domain. SARIF works for any domain with file+line findings.

2. **Input/output are decoupled.** The same review can run on a GitHub PR, a
   local file, or a piped diff. The same result can be posted to GitHub,
   emitted as SARIF, or returned over MCP.

3. **Domains are files.** Adding a domain is `mkdir prompts/foo && vim
   prompts/foo/system.txt`. No code. No config changes.

4. **IO surfaces are thin.** The CLI, Action, webhook, and MCP server are
   adapters around `ReviewRequest → ReviewResult`. No business logic lives in
   them.

5. **Secrets never leak.** `Sensitive<T>` at the type level. Secrets can't
   appear in logs, config dumps, or AI prompts.

---

## v2 candidates (not yet planned)

- **SQLite/Dolt backend** for review history, session persistence,
  multi-agent coordination (not git operations — that's a separate concern)
- **Multi-provider** via extracted `GitProvider` trait (GitLab, Bitbucket)
- **GitHub Checks API** output (in addition to PR comments)
- **Webhook server** polish (queue, retry, status hooks)
- **Accurate token counting** via `tiktoken-rs`
- **Incremental reviews** (track last reviewed commit)
