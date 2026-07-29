//! MCP tool definitions and handler functions.

use crate::engine::{ReviewEngine, ReviewOptions, ReviewRequest, ReviewSource};
use crate::github::parse_pr_url;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

// ── Input structs for each tool ───────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewPrArgs {
    pub pr_url: String,
    #[serde(default = "default_post")]
    pub post: bool,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub extra_instructions: String,
    /// If true, update the previous review comment in place.
    #[serde(default)]
    pub sticky: bool,
    /// If true, enable the LLM tool loop during review.
    #[serde(default)]
    pub use_tools: bool,
    /// Maximum time in seconds to wait for the AI review (default: no limit).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn default_post() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewDiffArgs {
    pub diff: String,
    pub title: String,
    #[serde(default = "default_domain")]
    pub domain: String,
    #[serde(default)]
    pub language: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub extra_instructions: String,
    /// Output format: "reviewer" (default) or "sarif"
    #[serde(default = "default_format")]
    pub format: String,
    /// Maximum time in seconds to wait for the AI review.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn default_format() -> String {
    "reviewer".into()
}

fn default_domain() -> String {
    "code".into()
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewFilesArgs {
    pub pr_url: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub post: bool,
    #[serde(default)]
    pub extra_instructions: String,
    /// Maximum time in seconds to wait for the AI review.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewFileArgs {
    pub path: String,
    #[serde(default = "default_domain")]
    pub domain: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub extra_instructions: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewGlobArgs {
    pub pattern: String,
    #[serde(default = "default_domain")]
    pub domain: String,
    #[serde(default)]
    pub extra_instructions: String,
    /// Maximum time in seconds to wait for the AI review.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

// ── Tool definitions ──────────────────────────────────────────

/// All tools that this server exposes.
pub(crate) fn tool_definitions() -> Vec<(String, String, Value)> {
    vec![
        (
            "review_pr".into(),
            "Review a GitHub pull request. Fetches the diff, analyzes it with the AI, and optionally posts the review as a comment on the PR. Supports GitHub, GitLab, and Bitbucket URLs.".into(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pr_url": { "type": "string", "description": "Full PR URL — e.g. https://github.com/owner/repo/pull/42, https://gitlab.com/o/r/-/merge_requests/7, or https://bitbucket.org/o/r/pull-requests/99" },
                    "post": { "type": "boolean", "description": "Whether to post the review to GitHub as a PR comment", "default": true },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Only review files matching these path prefixes", "default": [] },
                    "extra_instructions": { "type": "string", "description": "Extra context injected into the review prompt", "default": "" },
                    "timeout_secs": { "type": "number", "description": "Maximum seconds to wait for AI review", "default": 0 }
                },
                "required": ["pr_url"]
            }),
        ),
        (
            "review_diff".into(),
            "Review a raw unified diff string without fetching from GitHub. Use for ad-hoc code review, pre-fetched diffs, or working-tree changes.".into(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "diff": { "type": "string", "description": "Raw unified diff text (git diff output)" },
                    "title": { "type": "string", "description": "A short title describing the change" },
                    "domain": { "type": "string", "description": "Review domain: 'code', 'config', 'policy', 'design', 'data'", "default": "code" },
                    "language": { "type": "string", "description": "Primary programming language hint (e.g. 'Rust', 'Python'). Only used in 'code' domain.", "default": "Unknown" },
                    "description": { "type": "string", "description": "Optional longer description of the change" },
                    "extra_instructions": { "type": "string", "description": "Extra context injected into the review prompt", "default": "" },
                    "format": { "type": "string", "description": "Output format: 'reviewer' (default) or 'sarif'", "default": "reviewer" },
                    "timeout_secs": { "type": "number", "description": "Maximum seconds to wait for AI review", "default": 0 }
                },
                "required": ["diff", "title"]
            }),
        ),
        (
            "review_files".into(),
            "Review only specific files from a pull request (GitHub, GitLab, Bitbucket). Useful when the AI wants to focus on relevant changes after inspecting the PR's file list.".into(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pr_url": { "type": "string", "description": "Full PR URL — e.g. https://github.com/owner/repo/pull/42, https://gitlab.com/o/r/-/merge_requests/7, or https://bitbucket.org/o/r/pull-requests/99" },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Only review files matching these path prefixes" },
                    "post": { "type": "boolean", "description": "Whether to post the review to GitHub", "default": false },
                    "extra_instructions": { "type": "string", "description": "Extra context injected into the review prompt", "default": "" },
                    "timeout_secs": { "type": "number", "description": "Maximum seconds to wait for AI review", "default": 0 }
                },
                "required": ["pr_url", "paths"]
            }),
        ),
        (
            "review_file".into(),
            "Review a single file from the filesystem. Path must be relative and must not contain '..'.".into(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to review (relative to CWD)" },
                    "domain": { "type": "string", "description": "Review domain", "default": "code" },
                    "language": { "type": "string", "description": "Optional language override" },
                    "description": { "type": "string", "description": "Optional context for the review" },
                    "extra_instructions": { "type": "string", "description": "Extra context injected into the review prompt", "default": "" },
                    "timeout_secs": { "type": "number", "description": "Maximum seconds to wait for AI review", "default": 0 }
                },
                "required": ["path"]
            }),
        ),
        (
            "review_glob".into(),
            "Review all files matching a glob pattern. Patterns are relative to CWD.".into(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern, e.g. 'src/**/*.rs'" },
                    "domain": { "type": "string", "description": "Review domain", "default": "code" },
                    "extra_instructions": { "type": "string", "description": "Extra context injected into the review prompt", "default": "" },
                    "timeout_secs": { "type": "number", "description": "Maximum seconds to wait for AI review", "default": 0 }
                },
                "required": ["pattern"]
            }),
        ),
    ]
}

// ── Timeout helper ────────────────────────────────────────────

/// Run a review with an optional timeout. Returns the review result,
/// or a timeout error message if the review takes longer than `timeout_secs`.
async fn run_with_timeout(
    engine: &ReviewEngine,
    request: ReviewRequest,
    timeout_secs: Option<u64>,
) -> std::result::Result<Value, String> {
    let review_fut = async {
        let result = engine
            .review(request)
            .await
            .map_err(|e| format!("Review failed: {e}"))?;
        serde_json::to_value(&result).map_err(|e| format!("Failed to serialize result: {e}"))
    };

    match timeout_secs {
        Some(secs) if secs > 0 => tokio::time::timeout(Duration::from_secs(secs), review_fut)
            .await
            .map_err(|_| format!("Review timed out after {} seconds", secs))?,
        _ => review_fut.await,
    }
}

// ── Handler functions ─────────────────────────────────────────

/// Parse a `pr_url` string into owner/repo/number.
/// Accepts `https://github.com/owner/repo/pull/42` or `www.github.com` variant.
///
/// Accepts `https://github.com/owner/repo/pull/42` or `www.github.com` variant.
/// Handle the `review_pr` tool.
pub(crate) async fn handle_review_pr(
    engine: &ReviewEngine,
    params: Value,
) -> std::result::Result<Value, String> {
    let args: ReviewPrArgs =
        serde_json::from_value(params).map_err(|e| format!("Invalid arguments: {e}"))?;

    let (owner, repo, number) = parse_pr_url(&args.pr_url)?;

    let request = ReviewRequest {
        source: ReviewSource::PrUrl {
            owner,
            repo,
            number,
        },
        options: ReviewOptions {
            post_to_github: args.post,
            paths: args.paths,
            extra_instructions: args.extra_instructions,
            sticky: args.sticky,
            use_tools: args.use_tools,
            resume_session: None,
        },
    };

    run_with_timeout(engine, request, args.timeout_secs).await
}

/// Handle the `review_diff` tool.
pub(crate) async fn handle_review_diff(
    engine: &ReviewEngine,
    params: Value,
) -> std::result::Result<Value, String> {
    let args: ReviewDiffArgs =
        serde_json::from_value(params).map_err(|e| format!("Invalid arguments: {e}"))?;

    let request = ReviewRequest {
        source: ReviewSource::DiffText {
            diff: args.diff,
            title: args.title,
            domain: args.domain,
            language_hint: args.language.unwrap_or_default(),
            description: args.description,
        },
        options: ReviewOptions {
            post_to_github: false,
            paths: Vec::new(),
            extra_instructions: args.extra_instructions,
            sticky: false,
            use_tools: false,
            resume_session: None,
        },
    };

    if args.format == "sarif" {
        let result = engine
            .review(request)
            .await
            .map_err(|e| format!("Review failed: {e}"))?;
        Ok(crate::sarif::to_sarif_value(&result))
    } else {
        run_with_timeout(engine, request, args.timeout_secs).await
    }
}

/// Handle the `review_file` tool.
pub(crate) async fn handle_review_file(
    engine: &ReviewEngine,
    params: Value,
) -> std::result::Result<Value, String> {
    let args: ReviewFileArgs =
        serde_json::from_value(params).map_err(|e| format!("Invalid arguments: {e}"))?;

    let request = ReviewRequest {
        source: ReviewSource::File {
            path: args.path,
            domain: args.domain,
            language: args.language,
            description: args.description,
        },
        options: ReviewOptions {
            post_to_github: false,
            paths: Vec::new(),
            extra_instructions: args.extra_instructions,
            sticky: false,
            use_tools: false,
            resume_session: None,
        },
    };

    run_with_timeout(engine, request, args.timeout_secs).await
}

/// Handle the `review_glob` tool.
pub(crate) async fn handle_review_glob(
    engine: &ReviewEngine,
    params: Value,
) -> std::result::Result<Value, String> {
    let args: ReviewGlobArgs =
        serde_json::from_value(params).map_err(|e| format!("Invalid arguments: {e}"))?;

    let request = ReviewRequest {
        source: ReviewSource::Glob {
            pattern: args.pattern,
            domain: args.domain,
        },
        options: ReviewOptions {
            post_to_github: false,
            paths: Vec::new(),
            extra_instructions: args.extra_instructions,
            sticky: false,
            use_tools: false,
            resume_session: None,
        },
    };

    run_with_timeout(engine, request, args.timeout_secs).await
}

/// Handle the `review_files` tool.
pub(crate) async fn handle_review_files(
    engine: &ReviewEngine,
    params: Value,
) -> std::result::Result<Value, String> {
    let args: ReviewFilesArgs =
        serde_json::from_value(params).map_err(|e| format!("Invalid arguments: {e}"))?;

    if args.paths.is_empty() {
        return Err("'paths' must be a non-empty array of path prefixes".into());
    }

    let (owner, repo, number) = parse_pr_url(&args.pr_url)?;

    let request = ReviewRequest {
        source: ReviewSource::PrUrl {
            owner,
            repo,
            number,
        },
        options: ReviewOptions {
            post_to_github: args.post,
            paths: args.paths,
            extra_instructions: args.extra_instructions,
            sticky: false,
            use_tools: false,
            resume_session: None,
        },
    };

    run_with_timeout(engine, request, args.timeout_secs).await
}
