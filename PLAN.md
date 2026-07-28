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
AI prompts. Adapted from OCR's rule system.

### Design

```
prompts/{domain}/rules/
├── rules.json        ← index file (pattern → file mapping)
├── rust.md           ← **/*.rs
├── python.md         ← **/*.py
├── rust-security.md  ← **/*.rs  (cross-cutting, stacked with rust.md)
├── yaml.md           ← **/*.yaml
└── ...
```

**Built-in rules** — shipped in `prompts/{domain}/rules/` and embedded via
`include_str!`. Initially ~10 covering the most common languages, extended
over time toward OCR's ~27.

**Index format:**

```json
[
  {"pattern": "**/*.rs",      "files": ["rust.md", "security.md"]},
  {"pattern": "**/*.py",      "files": ["python.md"]},
  {"pattern": "**/*.yaml",    "files": ["yaml.md"]},
  {"pattern": "**/Dockerfile", "files": ["docker.md"]}
]
```

Multiple `files` per entry enables stacking (e.g., Rust + security).
First-match-wins per pattern, but a single match loads all listed files.

**Per-project custom rules** — scanned from working directory:

```
.reviewer/rules.json    ← optional, merged after built-in rules
```

If present, its entries extend (not replace) the built-in set. This gives
teams a mechanism to add project-specific rules without modifying shipped
prompt files. Same schema as the built-in index.

**Injection:**
- New `{system_rule}` placeholder in system prompt templates
- `PromptBuilder` resolves rules per-file and injects matched content
- Multiple rule files can combine (e.g., language + security + policy)

**What changes:**
| File | Change |
|---|---|
| `src/services/prompt_builder.rs` | `resolve_rules(domain, files) -> Vec<String>`, inject via `{system_rule}` |
| `prompts/code/system.txt` | Add `{system_rule}` placeholder |
| `prompts/config/system.txt` | Same |
| `src/engine.rs` | Pass resolved rules through to PromptBuilder |
| `prompts/code/rules/` | Initial rule files + index |
| `prompts/config/rules/` | Config-domain rule files (schema, security baseline) |

### Testing
- Unit tests: rule resolution by pattern, rule stacking, missing index,
  empty domain, index with non-existent files
- Integration: full pipeline with rules injected, verify prompt contains
  rule content

---

## Phase 1b — Per-repo config

**Goal:** Allow teams to customize reviewer behavior per repository without
modifying `action.yml`. Adapted from OCR's `.pr_agent.toml` and PR-Agent's
`.pr_agent.toml` patterns.

### Design

The config search path already includes
`$GITHUB_WORKSPACE/.github/reviewer.toml` (first position) but no feature
uses it. This phase wires it up:

- If `.github/reviewer.toml` exists (or `.reviewer/config.toml`), merge it
  into settings with the same env-overlay logic
- Per-repo overrides are limited to a safe subset: `review.extra_instructions`,
  `review.max_input_tokens`, `review.max_diff_files`, `ai.model`,
  `ai.temperature` — nothing that affects binary behavior or security

**What changes:**
| File | Change |
|---|---|
| `src/config.rs` | Document the per-repo search path, validate overridable keys |

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
pub struct LocalRepo { repo: gix::Repository }

impl LocalRepo {
    pub fn open(path: &str) -> Result<Self>;
    pub fn diff_between(&self, base: &str, head: &str) -> Result<String>;
    pub fn file_at(&self, commit: &str, path: &str) -> Result<String>;
    pub fn grep(&self, pattern: &str, paths: &[&str]) -> Result<Vec<Match>>;
}
```

**Hybrid read path:**
- `ReviewSource::PrUrl` checks if `--repo-path` was provided
- If yes: `LocalRepo` computes diff + file content locally (zero API calls)
- If no: fall back to GitHub API (CI ephemeral runners)
- GitHub API always used for **posting**

**New CLI flags:** `--repo-path`, `--base-ref`, `--head-ref`

**What changes:**
| File | Change |
|---|---|
| `Cargo.toml` | Add `gix = "0.86"` |
| `src/git/mod.rs` | New |
| `src/git/local.rs` | New |
| `src/git/search.rs` | New |
| `src/engine.rs` | Hybrid read in source resolution |
| `src/main.rs` | CLI flags |
| `src/mcp/tools.rs` | `repo_path` optional param on existing tools |
| `src/config.rs` | `[git] repo_path` setting |

### Testing
- Unit: LocalRepo diff/file_at/grep with known repos
- Integration: wiremock for GitHub fallback path, real filesystem for gix path
- No network in CI (all git operations on test fixtures)

---

## Phase 3 — LLM tool loop + accurate token counting

**Goal:** Replace the single-shot AI call with an interactive loop where
the AI can call tools to gather more context before producing findings.

This is the highest-impact change. It also requires accurate token counting
to manage memory compression correctly — the current 3.5 chars/token
heuristic is too imprecise for the 60%/80% compression thresholds.

### Prerequisite: accurate token counting

Switch from heuristic to `tiktoken-rs` (pure Rust, no C deps):

```rust
use tiktoken_rs::cl100k_base;

pub fn estimate_tokens(text: &str) -> usize {
    let bpe = cl100k_base().unwrap();
    bpe.encode_with_special_tokens(text).len()
}
```

This changes the token budget calculation throughout the pipeline — input
estimates, overhead calculations, truncation decisions all become accurate.

### Core change: tool loop

```
1. Build system + user prompt (as today)
2. Send to AI with tool definitions attached
3. If AI calls a tool:
   a. Execute tool
   b. Append result to conversation
   c. Send back to AI
   d. Repeat
4. If AI calls task_done → exit loop
5. Max rounds configurable (default 10)
```

**Tool definitions (per-domain):**

| Tool | Code domain | Config domain | Policy domain |
|---|---|---|---|
| `file_read` | ✅ | ✅ | ✅ |
| `code_search` | ✅ | ❌ | ❌ |
| `file_find` | ✅ | ✅ | ✅ |
| `file_read_diff` | ✅ | ❌ | ❌ |
| `submit_finding` | ✅ | ✅ | ✅ |
| `task_done` | ✅ | ✅ | ✅ |

**Architecture:**

```rust
#[async_trait]
trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;
    async fn execute(&self, args: serde_json::Value) -> Result<String>;
}
```

**AI client changes** (understated in v1 of this plan):

The current `AiClient::chat()` sends `[{role:"system"},{role:"user"}]` and
receives `{role:"assistant", content:"..."}`. Tool calls require:

- **Request side:** new `tools` field on the chat request body. Each tool
  has a name, description, and JSON schema for arguments.
- **Response side:** detect `finish_reason: "tool_calls"`. The response
  carries `tool_calls: [{id, type, function: {name, arguments}}]` instead
  of (or alongside) `content`.
- **Conversation append:** after executing a tool, append
  `{role: "tool", tool_call_id, content}` to the message list.
- **New types in `src/ai/types.rs`:** `ToolDef`, `ToolCall`, `ToolResult`.

**Memory compression** (adapted from OCR):
- Track total tokens per round using accurate `tiktoken` counts
- At 60% of `max_input_tokens`: trigger background compression
- At 80%: trigger synchronous compression
- Compression: summarize older rounds into a `<previous_review_summary>`
  appendix on the user message

**What changes:**
| File | Change |
|---|---|
| `Cargo.toml` | Add `tiktoken-rs` |
| `src/tokens.rs` | Replace heuristic with `tiktoken` |
| `src/ai/types.rs` | Add `ToolDef`, `ToolCall`, `ToolResult`, updated `ChatRequest`, `ChatOutput` |
| `src/ai/mod.rs` | Support tool-bearing requests, tool-call responses, finish_reason branching |
| `src/engine.rs` | Add tool loop, ToolRegistry, tool dispatch, compression |
| `src/tools/mod.rs` | Tool trait, registry, domain-specific tool lists |
| `src/tools/file_read.rs` | New |
| `src/tools/code_search.rs` | New (uses gix — code domain only) |
| `src/tools/file_find.rs` | New |
| `src/tools/file_read_diff.rs` | New |
| `src/tools/submit_finding.rs` | New |
| `src/tools/task_done.rs` | New |
| `prompts/code/system.txt` | Add tool descriptions and schemas |
| `prompts/config/system.txt` | Same (config-domain tools only) |
| `src/config.rs` | `max_tool_rounds` setting, tool enable/disable per domain |

### Testing
- Unit: ToolRegistry construction, tool dispatch, tool execute
- Unit: `tiktoken` vs heuristic comparison (validate no regressions in
  budget behavior)
- Integration: wiremock serves tool-call responses, verify loop executes
  tools and returns combined result

---

## Phase 4 — Post-hoc accuracy improvements

**Goal:** Reduce false positives and fix misaligned line numbers after the
main review loop. Works with both single-shot and tool-loop modes.

### Review filter (FP detection)

After findings are collected, send them to the AI for verification:

```rust
fn review_filter(findings: &[Finding], diff: &str) -> Result<Vec<Finding>> {
    let prompt = format!(
        "Given this diff:\n```\n{diff}\n```\n\n\
         Which of these findings are incorrect?\n\
         {findings_json}\n\n\
         Return only the indices of incorrect findings as a JSON array."
    );
    let response = ai.chat(&system_prompt, &prompt).await?;
    parse_indices(&response)
}
```

### Re-location (line number correction)

When a finding has no valid line numbers, attempt to match `existing_code`
against the diff through three passes:

1. Direct hunk matching (normalized string comparison, new-side then old-side)
2. Full file content scanning (sliding window, skipping blank lines)
3. LLM re-location (send diff + existing_code to AI, extract corrected snippet,
   retry matching)

Passes 1 and 2 need no AI call and are fast. Pass 3 is a fallback.

**What changes:**
| File | Change |
|---|---|
| `src/engine.rs` | Post-processing step after AI analysis (always runs) |
| `src/services/review_filter.rs` | New |
| `src/services/relocation.rs` | New |
| `src/diff.rs` | Add `resolve_line_numbers()`, `normalize_line()`, `match_consecutive()` |

### Testing
- Unit: line matching with known hunks, edge cases (renamed, binary, empty)
- Unit: review filter with known FP/FN cases
- Integration: full pipeline with review filter, verify FP removed

---

## Phase 5 — Sticky summaries & history (GitHub posting)

**Goal:** Replace one-shot comment posting with updatable, history-aware
comments. GitHub-only — MCP returns results directly and doesn't need
this.

### Sticky review comment

Find existing comment by marker → edit in place:

```
Run 1: "## Review (initial analysis)"
Run 2: locate comment by HTML marker → edit → "## Review (updated)"
```

### History accumulation

```
## Review (updated)

(new findings table)

<details>
<summary>Previous results (2 earlier runs)</summary>

(run 2 table)
(run 1 table)
</details>
```

**What changes:**
| File | Change |
|---|---|
| `src/github/mod.rs` | `find_comment(marker)`, `edit_comment(id, body)` |
| `src/github/types.rs` | Comment search/edit types |
| `src/services/github_service.rs` | Sticky logic, optional history |
| `src/engine.rs` | Pass run counter for history |

### Testing
- Wiremock tests for comment search, edit, create
- Integration: verify marker-based find/edit cycle

---

## Phase 6 — Per-file concurrency

**Goal:** Review multiple files in parallel instead of sequentially.

### Challenge

The current `ReviewEngine::review()` processes all files in a single
synchronous pipeline:

```
1. Resolve source (single diff or file list)
2. Build single prompt from ALL files
3. Single AI call
4. Post single result
```

For concurrency, this must become:

```
Resolve source → split into per-file requests
                    │
       ┌────────────┼────────────┐
       ▼            ▼            ▼
  review file   review file  review file
  (sub-engine)  (sub-engine) (sub-engine)
       │            │            │
       └────────────┼────────────┘
                    ▼
              merge results
```

This means the engine needs two modes:
- **Batch mode** (current): all files in one prompt — cheaper, sees
  cross-file context
- **Per-file mode** (new, concurrent): each file independently —
  costlier but parallel, enables the tool loop per file

The `ReviewSource` determines which mode applies:
- PRs with small diffs → batch mode (current, unchanged)
- PRs with large diffs or file/glob sources → per-file concurrent mode

**What changes:**
| File | Change |
|---|---|
| `src/engine.rs` | Split into `review_batch()` and `review_per_file()`; dispatch with semaphore |
| `src/config.rs` | `review.max_concurrent_files` (default 4) |
| `src/tokens.rs` | Per-file budget accounting |
| `src/error.rs` | Error aggregation (partial failure semantics) |

### Testing
- Integration: wiremock with multiple files, verify concurrent execution
- Unit: error aggregation, semaphore behavior

---

## Phase 7 — Session persistence & resume

**Goal:** Allow interrupted reviews to resume without re-analyzing
completed files. Adapted from OCR.

### Design

```
.reviewer/sessions/<session-id>.jsonl

Record types: session_start, review_item_done, review_item_failed,
              review_item_reused, session_end
```

Each file gets a SHA-256 fingerprint of mode + path + diff. On resume,
already-analyzed files are skipped and their prior comments replayed.

`--resume <session-id>` flag on CLI and MCP.

**What changes:**
| File | Change |
|---|---|
| `src/session/mod.rs` | New module |
| `src/session/history.rs` | Session types, fingerprint |
| `src/session/persist.rs` | JSONL writer |
| `src/engine.rs` | Record progress, check resume state |

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
   ├── Phase 1b ── Per-repo config (no dependencies)
   │
   ├── Phase 2 ─── gix (no dependencies)
   │
   ├── Phase 3 ─── LLM tool loop + accurate token counting
   │   └── optional: Phase 2 for richer code tools
   │
   ├── Phase 4 ─── Post-hoc accuracy
   │   └── no dependency on Phase 3 (works with single-shot)
   │
   ├── Phase 5 ─── Sticky summaries
   │   └── no dependencies
   │
   ├── Phase 6 ─── Concurrency
   │   └── no dependencies
   │
   ├── Phase 7 ─── Session persistence
   │   └── depends on Phase 6 (records per-file state)
   │
   └── Phase 8 ─── New domains
       └── no dependencies
```

Phases 1, 1b, 2, 4, 5, 6 are fully parallelizable. Phase 3 is the only
phase with a soft dependency (gix for richer tools) and the most complex.

---

## Effort & impact matrix

| Phase | Effort | Impact | Risk | Dependencies |
|---|---|---|---|---|
| 1 — Rules | Low | High | Low | None |
| 1b — Per-repo config | Very low | Medium | Low | None |
| 2 — gix | Medium | Medium | Low | None |
| 3 — Tool loop | High | Very high | Medium | Optional: Phase 2 |
| 4 — Post-hoc | Medium | Medium | Low | None |
| 5 — Sticky | Low | Low | Low | None |
| 6 — Concurrency | Medium | High | Medium | None |
| 7 — Session | Medium | Low | Low | Phase 6 |
| 8 — New domains | Low | High (demo) | None | None |

**Recommended order:**
Ground layer (parallel): 1, 1b, 2, 5, 6
Value layer: 3, 4
Polish: 7, 8

---

## Appendix: OCR patterns explicitly not adopted

| OCR pattern | Why not |
|---|---|
| 4-layer rule priority (--flag > project > global > system) | Project-level `.reviewer/rules.json` is simpler and sufficient. |
| Async CommentWorkerPool | Our `submit_finding` tool is synchronous — the AI waits for the result. Thread-safety cost outweighs benefit. |
| XML message format for compression | JSON avoids XXE concerns and is consistent with the rest of the stack. |
| DiffMap snapshot for file_read_diff | Our tool loop can re-read from gix or filesystem. No snapshot needed. |
| ANSI terminal output with inline diff | Low priority. JSON + SARIF cover our users' needs. Terminal can be improved later. |
| Per-file timeout in goroutines | Phase 6 concurrency will use `tokio::time::timeout` per file, adopted from OCR. |

---

## Appendix: Testing strategy per phase

| Phase | Approach |
|---|---|
| 1 — Rules | Unit tests for rule resolution, pattern matching, stacking. Integration tests verify prompt contains injected rule text. |
| 1b — Config | Unit tests for config merge, key validation, env overlay. |
| 2 — gix | Unit tests against known git fixtures. No network. |
| 3 — Tool loop | Wiremock serves tool-call responses. Verify loop executes tools, compresses memory, produces findings. Separate unit tests for TokenHandler, ToolRegistry, each tool. |
| 4 — Post-hoc | Unit tests for line matching algorithms. Wiremock tests for filter and re-location AI calls. |
| 5 — Sticky | Wiremock tests for comment search/edit/create with markers. |
| 6 — Concurrency | Wiremock with multi-file PR. Verify concurrent execution, result merging, partial failure. |
| 7 — Session | Tempfile for session files. Verify write, resume, replay. |
| 8 — Domains | Integration tests with each new domain's prompt files. Verify pipeline runs end-to-end. |
