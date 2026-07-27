# Domain-Agnostic Reviewer Engine

## Core Insight

The pipeline is already domain-agnostic. Only the prompts, language detector, and source adapters are domain-specific:

```
input → parse → filter → budget → prompt → AI → sanitize → format → output
```

## Architecture

```
                    ┌──────────────────────────────────┐
                    │         reviewer role             │
                    │  "review this change set"         │
                    │  input: diff + context            │
                    │  output: structured findings      │
                    └──────────────────────────────────┘

  ┌───────────────────┐     ┌──────────────────┐     ┌──────────────────┐
  │  Source Adapters  │     │  Domain Profiles  │     │ Output Channels  │
  │                   │     │                   │     │                  │
  │ • git PR URL     │──▶  │ • code            │──▶  │ • MCP (agent)    │
  │ • stdin/pipe     │     │ • config/review   │     │ • stdout (CLI)   │
  │ • raw diff       │     │ • policy/audit    │     │ • SARIF (CI)     │
  │ • (future: file) │     │ • design/spec     │     │ • JSON (tooling) │
  │                   │     │ • data/schema     │     │ • PR comment     │
  └───────────────────┘     └──────────────────┘     └──────────────────┘
```

## Layer 1: Domain Profiles

Each domain gets its own prompt pair in `prompts/{domain}/`:

```
prompts/
├── code/
│   ├── system.txt    ← current review_system.txt (moved verbatim)
│   └── user.txt      ← current review_user.txt (moved verbatim)
├── config/
│   ├── system.txt    ← "Review this configuration change for correctness..."
│   └── user.txt
├── policy/
│   ├── system.txt    ← "Audit this policy document for gaps..."
│   └── user.txt
├── design/
│   ├── system.txt    ← "Review this design specification..."
│   └── user.txt
└── data/
    ├── system.txt    ← "Review this data migration for integrity..."
    └── user.txt
```

**Changes:**
- `PromptBuilder` loads from `prompts/{domain}/` at runtime, with fallback chain:
  `{domain}/system.txt` → `code/system.txt` → built-in compiled default
- Default domain = `"code"` — backward compatible
- `language` param → `domain` param in `review_diff` tool
- `language.rs` becomes optional — only loaded for `code` domain
- Each response carries `prompt_version: "{domain}/{revision}"` (e.g. `"code/1"`)

## Layer 2: Source Adapters (v1 scope only)

Extend `ReviewSource` enum. Each adapter produces `raw_diff: String` — engine unchanged.

| Adapter | Input | v1? | Notes |
|---------|-------|------|-------|
| `GitPr { url, post }` | Any git URL | ✅ | URL parser generalizes to GitLab/Bitbucket in v2 |
| `DiffText { diff, title }` | Raw diff string | ✅ | Already exists |
| `Stdin { title }` | Pipe | ✅ | `git diff \| reviewer review --stdin --format json` |
| `GitWorktree { path }` | Local repo | ❌ v2 | Needs `git` subprocess; security sandbox required |
| `LocalFile { path }` | Single file | ❌ v2 | No baseline to diff against |
| `Attachment { data }` | Base64 blob | ❌ v2 | MIME detection + format-specific decoders |

**Security:** URL parser uses a host allowlist (github.com, gitlab.com, bitbucket.org). No filesystem scanning in MCP mode (prevents path traversal from agents).

## Layer 3: Structured Findings

**This is the single most impactful change and the most underestimated engineering challenge.**

### Why it matters

- G-Research: "Treat LLM output as unverified input. Validate against a source of truth."
- Cloudflare: Uses a **coordinator agent** to deduplicate findings, judge severity, post structured comments.
- Without structured findings, downstream tools can only consume flat markdown.

### Risks (documented from industry experience)

| Risk | Mitigation |
|------|------------|
| **Truncation** — JSON cut off mid-response (finish_reason: "length") | Detect via API field, retry with halved max_tokens (max 1 retry) |
| **Malformed JSON** — trailing commas, unescaped chars, no code fence | Robust parser + 1 repair pass sending validation errors back to model |
| **Schema violations** — wrong field names, missing required fields | Validate against `ReviewFinding` struct, reject individual malformed findings |
| **Hallucinated findings** — model invents issues | Optional cross-reference against a rules index (v2). Separator: findings from review_text — text may have issues that structured parsing missed |
| **Two-pass verification** — false positives degrade trust | G-Research pattern: two simple passes outperform one complex prompt. Deferred to v2. |

### Architecture

```
AI prompt ──→ raw response
                   │
          ┌────────▼────────┐
          │  finish_reason   │  ← "length"? → retry with halved max_tokens
          │  check           │
          └────────┬────────┘
                   │
          ┌────────▼────────┐
          │  JSON extractor  │  ← handles code-fence wrapping, truncation
          └────────┬────────┘
                   │
          ┌────────▼────────┐
          │  Schema validator │  ← validates against ReviewFinding schema
          └────────┬────────┘
                   │
          ┌────────▼────────┐
          │  Repair pass (1×)│  ← send validation errors back to model
          └────────┬────────┘
                   │
          ┌────────▼────────┐
          │  Fallback        │  ← findings=[], review_text=full markdown
          └─────────────────┘
```

**The review is never blocked on JSON parsing failure.** Markdown is the fallback. JSON is a best-effort enhancement.

### Output format

Current:
```json
{"review_text": "## 🔍 Issues\n\n### file.rs:42 — Security vulnerability..."}
```

Proposed:
```json
{
  "review_text": "## 🔍 Issues\n\n### src/auth.rs:42 — SQL injection...",
  "findings": [
    {
      "severity": "high",
      "file": "src/auth.rs",
      "line": 42,
      "category": "security",
      "message": "SQL injection via unsanitized input",
      "suggestion": "Use parameterized queries"
    }
  ],
  "metadata": {
    "domain": "code",
    "prompt_version": "code/1",
    "files_changed": 4,
    "files_reviewed": 4,
    "latency_ms": 31222,
    "input_tokens": 1066,
    "output_tokens": 2147,
    "model": "@cm/deepseek-v4-flash"
  }
}
```

## Layer 4: Tool Schema

Current `review_diff`:
```
language: "Rust"     ← implies code domain
```

Proposed:
```
diff:        "..."    ← required, unchanged
title:       "..."    ← required, unchanged
domain:      "code"   ← selects prompt set (default "code")
language:    "Rust"   ← optional, only used in code domain
description: "..."    ← unchanged
```

## Layer 5: Metadata in Every Result

Every tool response includes operation metadata so agents can self-optimize:
```json
{
  "latency_ms": 31222,
  "model": "@cm/deepseek-v4-flash",
  "input_tokens": 1066,
  "output_tokens": 2147,
  "domain": "code",
  "prompt_version": "code/1"
}
```

No separate "status" tool needed — metadata piggybacks on every response.

## SARIF Output Channel

SARIF (Static Analysis Results Interchange Format) is a [OASIS standard](https://sarifweb.azurewebsites.net/) with native viewers in VS Code, Visual Studio, and GitHub. The `findings[]` schema maps naturally:

| ReviewFinding field | SARIF field |
|-------------------|-------------|
| `file:line` | `result.locations[0].physicalLocation` |
| `category` | `result.ruleId` |
| `message` | `result.message.text` |
| `severity` | `result.level` (note/warning/error) |

Add `--format sarif` flag to CLI. Findings become the canonical intermediate representation; SARIF is one serialization format.

## Trust Model

**The LLM is an untrusted component.** All structured output must be validated:

1. **Structural validation** — JSON parses, matches schema, required fields present
2. **Severity capping** — model may inflate severity; cap at what the domain rules define
3. **Soft-drop on failure** — invalid findings are dropped individually, not block the review
4. **Two-pass verification** (v2) — separate recall and precision passes, compare results

## Implementation Priority

| Priority | Change | Effort | Risk | Value |
|----------|--------|--------|------|-------|
| **P0** | Metadata on every response | ~50 LOC | None | High |
| **P0** | `domain` param on `review_diff` | ~80 LOC | Low | High |
| **P1** | Domain profiles (restructure prompts) | ~10 new files + refactor | Low | Medium |
| **P1** | Structured findings with validation pipeline | ~300 LOC | Medium | High |
| **P2** | `Stdin` source adapter | ~50 LOC | Low | Medium |
| **P2** | SARIF output channel | ~200 LOC | Low | Medium |
| **P3** | Generalize URL parser beyond GitHub | ~150 LOC | Medium | Medium |
| **P4** | All other source adapters | ~300+ LOC | High | Low |

## Impact Summary

| Area | Change | Effort |
|------|--------|--------|
| `prompts/` | Split into `prompts/{domain}/` | Small |
| `PromptBuilder` | Runtime loading, fallback chain | Small |
| `ReviewResult` | Add `findings[]`, `prompt_version` | Small |
| `review_diff` tool | Add `domain` param, optional `language` | Small |
| MCP schemas | Update `inputSchema` | Small |
| `findings` pipeline | JSON extractor + validator + fallback | Medium |
| `prompts/` content | Write domain-specific prompts | Medium |
| SARIF formatter | New `src/sarif.rs` | Medium |
| `Stdin` adapter | New `ReviewSource` variant | Small |
| URL parser | Generalize beyond GitHub | Medium |

**Total: ~1,590 LOC across 11 new files and 16 modified files.**

## Pre-requisite Decisions

Before implementing:

1. **Findings format** — `{"findings": [...]}` as proposed? Or flat `[...]`? **(Proposed: nested)**
2. **`PromptBuilder::new()` signature** — `domain: &str` only? Or `domain: &str, settings: &Settings`? **(Proposed: domain only — settings access can be added later)**
3. **Domain prompts** — Write `config`/`policy`/`design`/`data` prompts now, or defer to separate PRs? **(Proposed: only `config` for Phase 2 deliverable)**
4. **Repair pass depth** — How many repair passes before fallback? **(Proposed: 1 — G-Research found diminishing returns after 1)**
5. **SARIF vs JSON-Lines vs both** — Output format for CLI `--format` flag? **(Proposed: `--format json` (default), `--format markdown`, `--format sarif`)**
