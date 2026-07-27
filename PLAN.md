# reviewer — Implementation Plan

Domain-agnostic AI review engine. This plan builds on the architecture
captured in `PROPOSAL.md`, incorporating patterns from OCR (Alibaba),
PR-Agent (CodiumAI), and GitLens.

---

## Guiding principles

1. **Domain agnosticism first.** Every feature must work for any domain
   (code, config, policy, compliance, etc.) unless explicitly domain-
   specific. Code-domain-only features are opt-in.

2. **Incremental delivery.** Each phase ships working software. No phase
   depends on a later phase. Users benefit from each phase independently.

3. **Single contract.** `ReviewRequest → ReviewResult` stays the sole
   contract across all IO surfaces (CLI, Action, webhook, MCP).

4. **Engine before surface.** Features are added to `ReviewEngine` first,
   then exposed through existing IO surfaces. New surfaces are never
   blocked by engine work.

5. **Ship then iterate.** Phase 0 (current code) is production-ready and
   already deployed. Each subsequent phase is a focused addition, not a
   rewrite.

---

## Phase 0 — Current state (shipped)

### Capabilities

| Area | Detail |
|---|---|
| **IO surfaces** | CLI (6 subcommands), Docker Action, webhook server, MCP server |
| **Input sources** | GitHub PR, raw diff, stdin pipe, local file, glob pattern |
| **Domains** | `code` and `config` (prompts as files in `prompts/{domain}/`) |
| **Output formats** | Reviewer JSON (structured findings + markdown), SARIF v2.1.0 |
| **MCP tools** | `review_pr`, `review_diff`, `review_files`, `review_file`, `review_glob` |
| **Pipeline** | Diff parsing (diffy) → file filtering (skip-list + path filter + budget truncation) → prompt building → AI chat → JSON extraction + repair → optional GitHub posting |
| **Security** | `Sensitive<T>` type-level secret redaction, path traversal protection, TOCTOU-safe step summary |
| **Config** | TOML + env vars, Sensitive<T> for keys |
| **Deployment** | Static binary (~20MB distroless Docker), single-binary CLI |
| **Testing** | 190 tests, wiremock for HTTP, tempfile for FS, clean clippy |

### Known gaps (unchanged from Phase 0)

| Gap | Impact |
|---|---|
| Single-shot AI call (no tool loop) | AI can't read files or search code |
| No rule/skills injection | Project-specific guidelines can't reach the AI |
| No post-hoc review filter | False positives pass through |
| No line number re-location | Line numbers can be wrong |
| No per-repo config | Can't customize per project |
| GitHub API only for diff fetching | Rate limited, no full file content |
| No comment dedup/idempotency | Duplicate reviews on re-run |
| Sequential file processing | No concurrency for multi-file reviews |

---

## Phase 1 — Rule system (domain-generic)

**Goal:** Inject domain/language/technology-specific review guidelines into
AI prompts. Adapted from OCR's rule system — simplified for v1.

### Design

```
prompts/{domain}/rules/
├── rust.md          # matches **/*.rs
├── python.md        # matches **/*.py
├── rust-security.md # matches **/*.rs  (cross-cutting, injected alongside rust.md)
└── yaml.md          # matches **/*.yaml
```

**Resolution:**
- Single layer: scan `prompts/{domain}/rules/` at startup
- Each file named `<name>.md` — matched by glob pattern embedded in filename
  (e.g., `rust.md` → `**/*.rs` from a sidecar `.rules.json` index)
- Alternative: `rules.json` index file alongside rule files:

```json
[
  {"pattern": "**/*.rs", "file": "rust.md"},
  {"pattern": "**/*.py", "file": "python.md"},
  {"pattern": "**/*.yaml", "file": "yaml.md"}
]
```

**Injection:**
- New `{system_rule}` placeholder in system prompt templates
- `PromptBuilder` resolves matched rules per-file and injects into prompt
- Multiple rules can match a single file (e.g., `rust.md` + `security.md`)
- OCR-style first-match-wins per-index, but multiple index entries can stack

**What changes:**
| File | Change |
|---|---|
| `src/services/prompt_builder.rs` | Add `resolve_rules(domain, files) -> Vec<String>`, inject via `{system_rule}` |
| `prompts/code/system.txt` | Add `{system_rule}` placeholder before output format section |
| `prompts/config/system.txt` | Same |
| `src/engine.rs` | Pass resolved rules through to PromptBuilder |
| `prompts/code/rules/` | Initial set of rule files (ported from OCR's embedded rules) |

### Open questions
- How many initial rule files to ship? Minimum: Rust, Python, Go, JS/TS,
  YAML, Dockerfile, Terraform (7). Full OCR parity: ~27.
- Index format: embedded `.rules.json` or filename-as-pattern convention?
  Filename convention is simpler but less flexible for multi-pattern rules.

---

## Phase 2 — gix: local git for the code domain

**Goal:** Replace GitHub API's limited diff endpoint with local git
operations for full file content, code search, and offline capability.

### Design

```
New module: src/git/
├── mod.rs     — pub use
├── local.rs   — LocalRepo wrapping gix::Repository
└── search.rs  — grep/code_search via gix tree traversal
```

**`LocalRepo` API:**

```rust
pub struct LocalRepo {
    repo: gix::Repository,
}

impl LocalRepo {
    pub fn open(path: &str) -> Result<Self>;
    pub fn diff_between(&self, base: &str, head: &str) -> Result<String>;
    pub fn file_at(&self, commit: &str, path: &str) -> Result<String>;
    pub fn grep(&self, pattern: &str, paths: &[&str]) -> Result<Vec<Match>>;
    pub fn mergebased_diff(&self, base: &str, head: &str) -> Result<String>;
}
```

**New ReviewSource:**

```rust
pub enum ReviewSource {
    // ... existing variants ...
    LocalBranch {
        repo_path: String,
        base_ref: String,
        head_ref: String,
    },
}
```

**Code search tool** (MCP + CLI):

```rust
pub struct CodeSearchArgs {
    pub pattern: String,
    pub path: Option<String>,
}
```

**Hybrid read path:**
- `ReviewSource::PrUrl` checks if `--repo-path` was provided
- If yes: use `LocalRepo::diff_between()` for diff, `LocalRepo::file_at()` for
  full content → no GitHub API calls for reading
- If no: fall back to current GitHub API diff (CI ephemeral runners)
- GitHub API always used for **posting** reviews/comments

**What changes:**
| File | Change |
|---|---|
| `Cargo.toml` | Add `gix = "0.86"` |
| `src/git/mod.rs` | New module |
| `src/git/local.rs` | New |
| `src/git/search.rs` | New |
| `src/engine.rs` | New `ReviewSource::LocalBranch`, hybrid read in `resolve_source()` |
| `src/main.rs` | `--repo-path`, `--from-ref`, `--to-ref` flags |
| `src/mcp/tools.rs` | New `review_local_pr` tool (or extend `review_pr` with optional repo_path) |
| `src/config.rs` | Optional `[git] repo_path` setting |

### Non-goals (Phase 2)
- `code_search` LLM tool (deferred to Phase 3 tool loop)
- `file_read` tool (same)
- Per-key caching (single repo open at a time is fine for v1)

---

## Phase 3 — LLM tool loop (domain-generic engine)

**Goal:** Replace the single-shot AI call with an interactive loop where
the AI can call tools to gather more context before producing findings.

### Design

**Core change:** `ReviewEngine::review()` grows a loop:

```
1. Build system + user prompt (as today)
2. Send to AI with tool definitions attached
3. If AI calls a tool:
   a. Execute tool (file_read, code_search, etc.)
   b. Append result to conversation
   c. Send back to AI
   d. Repeat
4. If AI calls task_done → exit loop
5. If AI returns text without tool calls → that's the review
6. Max rounds configurable (default 10)
```

**Tool definitions (per-domain):**

| Tool | Code domain | Config domain | Policy domain |
|---|---|---|---|
| `file_read` | ✅ | ✅ | ✅ |
| `code_search` | ✅ | ❌ | ❌ |
| `file_find` | ✅ | ✅ | ✅ |
| `file_read_diff` | ✅ | ❌ | ❌ |
| `task_done` | ✅ | ✅ | ✅ |
| `submit_finding` | ✅ | ✅ | ✅ |

**Architecture:**

```rust
pub struct ReviewEngine {
    // Existing fields...
    tools: ToolRegistry,           // new
    max_tool_rounds: usize,        // new (default 10)
    max_consecutive_empty: usize,  // new (default 3)
}

struct ToolRegistry {
    domain: String,
    tools: HashMap<String, Box<dyn Tool>>,
}

#[async_trait]
trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    async fn execute(&self, args: serde_json::Value) -> Result<String>;
}
```

**Tool implementations:**

- **`file_read`**: reads from gix (code domain) or filesystem (other domains).
  Args: `path`, `start_line`?, `end_line`?. Max 500 lines.
- **`code_search`**: git grep via gix. Args: `pattern`, `case_sensitive?`,
  `file_patterns?`. Max 100 results.
- **`file_find`**: find files by name. Args: `name`, `case_sensitive?`.
  Max 100 results.
- **`file_read_diff`**: returns diff for a specific file from the parsed
  diff map. Args: `path`.
- **`submit_finding`**: submits a finding mid-review. Args: `severity`,
  `category`, `message`, `file`?, `line`?, `suggestion`?.
- **`task_done`**: signals completion. No args.

**Memory compression** (adapted from OCR):
- Track total tokens per conversation round
- At 60% of `max_input_tokens`: trigger background compression
- At 80%: trigger synchronous compression
- Compression: summarize older rounds into a `<previous_review_summary>`
  appendix on the user message

**What changes:**
| File | Change |
|---|---|
| `src/engine.rs` | Add tool loop, ToolRegistry, tool execution |
| `src/tools/mod.rs` | Tool trait, registry, domain-specific tool lists |
| `src/tools/file_read.rs` | New |
| `src/tools/code_search.rs` | New |
| `src/tools/file_find.rs` | New |
| `src/tools/submit_finding.rs` | New |
| `src/tools/task_done.rs` | New |
| `src/ai/mod.rs` | Support tool call request/response in chat API |
| `src/ai/types.rs` | Add `ToolCall`, `ToolResult` message types |
| `prompts/code/system.txt` | Add tool descriptions and schemas |
| `prompts/config/system.txt` | Same (config-domain tools only) |

### Open questions
- Should `code_comment` (OCR-style async finding submission) be supported
  alongside `submit_finding`? The sync version is simpler for v1.
- OCR uses `MaxToolRequestTimes` per file (default ~30). What's right for
  our token budget? 10 rounds with 16k tokens fits most use cases.
- OCR has async comment resolution via `CommentWorkerPool`. Do we need it
  in v1? Probably not — synchronous per-tool execution is simpler.

---

## Phase 4 — Post-hoc accuracy improvements

**Goal:** Reduce false positives and fix misaligned line numbers after the
main review loop.

### Review filter (FP detection)

After the tool loop completes (or even in single-shot mode), send collected
findings to the AI for verification:

```rust
fn review_filter(findings: &[Finding], diff: &str) -> Result<Vec<Finding>> {
    let prompt = format!(
        "Given this diff:\n```\n{diff}\n```\n\n\
         Which of these findings are incorrect?\n\
         {findings_json}\n\n\
         Return only the indices of incorrect findings as a JSON array."
    );
    let response = ai.chat(&system_prompt, &prompt).await?;
    parse_indices(&response).map(|indices| {
        findings.iter().enumerate()
            .filter(|(i, _)| !indices.contains(i))
            .map(|(_, f)| f.clone())
            .collect()
    })
}
```

### Re-location (line number correction)

When a finding has a suggestion but no valid line numbers, attempt to
match `existing_code` against the diff:

1. Try direct hunk matching (normalized string comparison)
2. Try full file content matching
3. If both fail, send to AI for re-location:

```rust
fn relocate(existing_code: &str, diff: &str) -> Result<(u64, u64)> {
    let prompt = format!(
        "Given this diff:\n```\n{diff}\n```\n\n\
         Find the exact lines for this code:\n```\n{existing_code}\n```\n\n\
         Return `start_line` and `end_line` as JSON."
    );
    // Parse response, update finding
}
```

**What changes:**
| File | Change |
|---|---|
| `src/engine.rs` | Post-processing step after AI analysis |
| `src/services/review_filter.rs` | New |
| `src/services/relocation.rs` | New (or extend existing diff resolver) |
| `src/diff.rs` | Add line-matching utilities (normalize, sliding window) |

---

## Phase 5 — Sticky summaries & history

**Goal:** Replace one-shot comment posting with updatable, history-aware
comments that follow PR-Agent's pattern.

### Sticky review comment

Instead of posting a new comment each run, find and update the existing one:

```
Run 1: Post comment "## 🔍 Review\n(initial review)"
Run 2: Find comment by marker → edit in place → "## 🔍 Review (updated)
        (updated review)"
```

### History accumulation

For inline code suggestions (future), maintain a growing `<details>` history:

```
## 🔍 Review (updated)

(new findings table)

<details>
<summary>Previous suggestions (2 earlier runs)</summary>

(run 2 table)
(run 1 table)
</details>
```

**What changes:**
| File | Change |
|---|---|
| `src/github/mod.rs` | Add `edit_comment()`, `find_comment()` API methods |
| `src/github/types.rs` | Add comment search/edit types |
| `src/services/github_service.rs` | Add sticky comment logic, optional history |
| `src/engine.rs` | Pass run context for history tracking |

---

## Phase 6 — Per-file concurrency

**Goal:** Review multiple files in parallel, with semaphore-throttled
goroutines (adapted from OCR).

### Design

```rust
// In ReviewEngine or a new Dispatcher
let semaphore = Semaphore::new(max_concurrent); // default 4

let handles: Vec<_> = files.iter().map(|file| {
    let permit = semaphore.acquire().await;
    tokio::spawn(async move {
        let _permit = permit;
        review_single_file(file).await
    })
}).collect();

let results = futures::future::join_all(handles).await;
```

**What changes:**
| File | Change |
|---|---|
| `src/engine.rs` | `review()` becomes concurrent for multi-file sources |
| `src/config.rs` | `review.max_concurrent_files` setting |
| `src/tokens.rs` | Budget accounting per-file (not per-batch) |

---

## Phase 7 — Session persistence & resume

**Goal:** Allow interrupted reviews to resume without re-analyzing
completed files (adapted from OCR).

### Design

- Session files stored in `.reviewer/sessions/<session-id>.jsonl`
- Each file's fingerprint (SHA-256 of mode + path + diff) tracked
- Replayed files restored from cache on resume
- `--resume <session-id>` flag on CLI and MCP

**What changes:**
| File | Change |
|---|---|
| `src/session/mod.rs` | New module |
| `src/session/history.rs` | Session types |
| `src/session/persist.rs` | JSONL writer |
| `src/engine.rs` | Record progress during review |

---

## Phase 8 — Net-new domains

**Goal:** Demonstrate domain agnosticism by adding real non-code domains.

### Candidate domains

| Domain | Use case | Rule examples |
|---|---|---|
| `policy` | Review PRs against organisational policy | `SECURITY.md`, `CODEOWNERS`, naming conventions |
| `compliance` | Audit config diffs against compliance frameworks | SOC2, PCI-DSS, HIPAA checklists |
| `data` | Review data files (CSV, JSON, protobuf) | Schema changes, PII detection, size budgets |
| `design` | Review architecture docs, ADRs | ADR format, decision criteria |
| `legal` | Review license files, dependency changes | License compatibility, copyright headers |

### What each new domain needs

```
prompts/{domain}/
├── system.txt    # Role definition, output format, DO NOT rules
├── user.txt      # Template with {diff}, {file_list}, etc.
└── rules/
    └── ...       # Domain-specific rule files
```

No code changes. No config changes.

---

## Dependency graph

```
Phase 0 ─── current (shipped)
   │
   ├── Phase 1 ─── Rule system (no dependencies)
   │
   ├── Phase 2 ─── gix (no dependencies)
   │
   ├── Phase 3 ─── LLM tool loop
   │   └── depends on Phase 2 (for file_read, code_search)
   │
   ├── Phase 4 ─── Post-hoc accuracy
   │   └── depends on Phase 3 (uses tool loop results)
   │
   ├── Phase 5 ─── Sticky summaries
   │   └── no dependency on Phases 2-4 (can ship independently)
   │
   ├── Phase 6 ─── Concurrency
   │   └── no dependency (can ship independently)
   │
   ├── Phase 7 ─── Session persistence
   │   └── depends on Phase 6 (records per-file results)
   │
   └── Phase 8 ─── New domains
       └── no dependency (only needs prompt files)
```

Phases 1, 2, 5, 6 can be done in parallel. Phase 3 is the most impactful
and most complex — it's the only phase that changes the core engine loop.

---

## Effort & impact matrix

| Phase | Effort | Impact | Risk | Dependencies |
|---|---|---|---|---|
| 1 — Rules | Low | High | Low | None |
| 2 — gix | Medium | Medium | Low | None |
| 3 — Tool loop | High | Very high | Medium | Phase 2 |
| 4 — Post-hoc | Medium | Medium | Low | Phase 3 |
| 5 — Sticky | Low | Low | Low | None |
| 6 — Concurrency | Medium | Medium | Low | None |
| 7 — Session | Medium | Low | Low | Phase 6 |
| 8 — New domains | Low | High (demo) | None | None |

**Recommended order:** 1 → 2 → 5 → 6 (parallel, low-risk) → 3 → 4 → 7 → 8

---

## Appendix: OCR patterns explicitly not adopted

| OCR pattern | Why not |
|---|---|
| 4-layer rule priority (--flag > project > global > system) | Overengineered for v1. Single `rules/` directory with index is sufficient. Users who need per-project overrides can use `reviewer.toml`'s `extra_instructions` field or create a custom domain. |
| Async CommentWorkerPool | Adds thread-safety complexity without clear benefit for our sync-first model. Our `submit_finding` tool is synchronous — the AI waits for the result. |
| XML message format for compression | JSON is simpler and avoids security concerns (XXE in downstream consumers). |
| DiffMap snapshot for file_read_diff | Our tool loop can re-read from gix or filesystem — no need for a separate snapshot mechanism. |
| Per-file fingerprint SHA-256 for resume | Useful but low urgency. Session persistence is Phase 7. |
| ANSI terminal output with inline diff | Low priority. Our JSON output is machine-readable; SARIF is CI-friendly. Terminal output can be improved later. |
