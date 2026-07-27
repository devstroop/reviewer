//! Core review engine: single `ReviewRequest → ReviewResult` contract
//! shared by all four IO surfaces (CLI, Action, webhook, MCP).
//!
//! Callers build a [`ReviewRequest`], call [`ReviewEngine::review()`],
//! and receive a [`ReviewResult`].  No caller touches parsing, prompting,
//! or GitHub internals.

use crate::ai::AiClient;
use crate::config::Settings;
use crate::error::{AgentError, Result};
use crate::services::file_reader::{self, FileContent};
use crate::services::{
    DiffService, GithubService, JsonExtractor, PromptBuilder, PromptContext,
    json_extractor::MIN_TOKENS_FOR_RETRY,
};
use crate::tokens::estimate_tokens;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tracing::info;

/// Baseline token overhead for prompt template text that is not part of the
/// diff context: headers, metadata fields, markdown formatting, diff fence
/// wrapping, and the file-list header.
const PROMPT_OVERHEAD_BASELINE: usize = 100;

/// Estimated tokens per file in the file-list section (the bullet point with
/// filename, status, and line counts).  Each entry is roughly 50-70 bytes.
const PROMPT_OVERHEAD_PER_FILE: usize = 20;

// ── Resolved source (private) ──────────────────────────────────

/// Strongly-typed result of resolving a `ReviewSource` into concrete data.
struct ResolvedSource {
    pr_number: Option<u64>,
    pr_title: Option<String>,
    description: Option<String>,
    raw_diff: String,
    file_contents: Option<Vec<FileContent>>,
    owner: Option<String>,
    repo: Option<String>,
    author: Option<String>,
    branch: Option<String>,
    base: Option<String>,
    domain: String,
    language_hint: Option<String>,
}

/// What to review and where it comes from.
#[derive(Clone)]
pub enum ReviewSource {
    /// Fetch from GitHub and post back.
    PrUrl {
        owner: String,
        repo: String,
        number: u64,
    },
    /// Review a raw diff string directly, without any GitHub interaction.
    DiffText {
        diff: String,
        title: String,
        domain: String,
        language_hint: String,
        description: Option<String>,
    },
    /// Read diff from stdin with a 5s timeout.
    Stdin {
        title: String,
        domain: String,
        language_hint: String,
    },
    /// Review a single file from the filesystem.
    File {
        path: String,
        domain: String,
        language: Option<String>,
        description: Option<String>,
    },
    /// Review all files matching a glob pattern.
    Glob { pattern: String, domain: String },
}

/// Behaviour flags for a single review invocation.
#[derive(Clone)]
pub struct ReviewOptions {
    /// Whether to post the review to GitHub (only meaningful for `PrUrl` sources).
    pub post_to_github: bool,
    /// If non-empty, only review files whose path starts with one of these prefixes.
    pub paths: Vec<String>,
    /// Extra instructions injected into the user prompt.
    pub extra_instructions: String,
}

impl Default for ReviewOptions {
    fn default() -> Self {
        Self {
            post_to_github: true,
            paths: Vec::new(),
            extra_instructions: String::new(),
        }
    }
}

/// Everything needed to run a single review.
#[derive(Clone)]
pub struct ReviewRequest {
    pub source: ReviewSource,
    pub options: ReviewOptions,
}

// ── Result types ───────────────────────────────────────────────

/// The complete result of a review invocation.
#[derive(Clone, Serialize)]
pub struct ReviewResult {
    pub review_text: String,
    pub findings: Vec<ReviewFinding>,
    pub pr_number: Option<u64>,
    pub pr_title: Option<String>,
    pub stats: ReviewStats,
}

/// A single structured finding extracted from the AI review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    /// Severity level: "high", "medium", "low", or "info".
    pub severity: String,
    /// File path relative to repo root (may be absent for project-wide findings).
    #[serde(default)]
    pub file: Option<String>,
    /// Line number in the file (1-based, optional).
    #[serde(default)]
    pub line: Option<u64>,
    /// Category such as "logic_error", "security", "performance".
    pub category: String,
    /// Human-readable description of the finding.
    pub message: String,
    /// Optional code suggestion.
    #[serde(default)]
    pub suggestion: Option<String>,
}

/// Statistics collected during a review run.
///
/// Note: `files_changed = files_skipped + files_path_filtered +
/// files_budget_dropped + files_reviewed`.  All four counters
/// are reported so callers can reconstruct total counts.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewStats {
    pub files_changed: usize,
    pub files_reviewed: usize,
    /// Files removed by the built-in skip-list OR the max_diff_files cap.
    /// Both filters are applied together inside `filter_files`, so this
    /// single counter covers both.  Path-filtered and budget-dropped
    /// files are tracked separately in `files_path_filtered` and
    /// `files_budget_dropped`.
    pub files_skipped: usize,
    /// Files removed by the caller-supplied `paths` filter.
    pub files_path_filtered: usize,
    /// Files dropped by token-budget truncation (largest files first).
    pub files_budget_dropped: usize,
    pub input_tokens_estimated: usize,
    /// Estimated system-prompt tokens (heuristic, same 3.5 chars/token).
    pub system_tokens_estimated: usize,
    pub output_tokens_reported: Option<u32>,
    /// Sum of system + input + output token estimates / reported values.
    ///
    /// Formula: `system_tokens_estimated + input_tokens_estimated + output_tokens_reported`.
    ///
    /// **Note for callers migrating from `ReviewOutput`:** the deprecated
    /// `ReviewOutput.total_tokens_used` omits system tokens (input + output
    /// only) to preserve legacy semantics.  `ReviewResult.stats.total_tokens_used`
    /// is the canonical field and includes all three segments.  If you need
    /// the old calculation for continuity, compute
    /// `input_tokens_estimated + output_tokens_reported.unwrap_or(0)`.
    pub total_tokens_used: Option<usize>,
    pub latency_ms: u64,
    /// AI model name used for this review.
    pub model: String,
    /// Prompt version (e.g. "1" initially, "{domain}/1" after Phase 1).
    pub prompt_version: String,
    /// Review domain (e.g. "code", "config", "policy").
    pub domain: String,
}

// ── Engine ─────────────────────────────────────────────────────

/// Core review engine.
///
/// Owns the services needed to fetch, parse, prompt, and analyse a PR diff.
/// Every IO surface adapts through this single `ReviewRequest → ReviewResult`
/// contract.
pub struct ReviewEngine {
    github_svc: Option<GithubService>,
    diff_svc: DiffService,
    ai: AiClient,
    /// Extra instructions from settings (merged into every user prompt).
    config_extra: String,
}

impl ReviewEngine {
    /// Construct a new engine from application settings.
    pub fn new(settings: &Settings) -> Result<Self> {
        let github_svc = if settings.github.token.inner().is_empty() {
            None
        } else {
            Some(GithubService::new(settings)?)
        };
        Ok(Self {
            github_svc,
            diff_svc: DiffService::new(settings),
            ai: AiClient::new(settings)?,
            config_extra: settings.review.extra_instructions.clone(),
        })
    }

    /// Run the full review pipeline.
    ///
    /// The flow depends on `ReviewSource`:
    /// - `PrUrl`: fetch metadata + diff from GitHub → parse → filter → path-filter → budget → prompt → AI → optionally post
    /// - `DiffText`: use the provided diff directly → parse → filter → path-filter → budget → prompt → AI
    pub async fn review(&self, request: ReviewRequest) -> Result<ReviewResult> {
        let start = Instant::now();
        let post_to_github = request.options.post_to_github;
        let path_filter = request.options.paths;

        // Merge settings-level extra_instructions with request-level ones.
        let extra = if self.config_extra.is_empty() {
            request.options.extra_instructions.trim().to_string()
        } else if request.options.extra_instructions.is_empty() {
            self.config_extra.trim().to_string()
        } else {
            format!(
                "{}\n\n{}",
                self.config_extra.trim(),
                request.options.extra_instructions.trim()
            )
        };

        // ── 1. Resolve source ───────────────────────────────────
        let resolved = match request.source {
            ReviewSource::PrUrl {
                owner,
                repo,
                number,
            } => {
                let o = owner.clone();
                let r = repo.clone();
                let github = self.github_svc.as_ref().ok_or_else(|| {
                    AgentError::Config(
                        "GITHUB_TOKEN is required to review PRs — set via env var or config file"
                            .into(),
                    )
                })?;
                let pr = github.get_pr_metadata(&o, &r, number).await?;
                let diff = github.get_pr_diff(&o, &r, number).await?;
                ResolvedSource {
                    pr_number: Some(number),
                    pr_title: Some(pr.title),
                    description: pr.body,
                    raw_diff: diff,
                    file_contents: None,
                    owner: Some(owner),
                    repo: Some(repo),
                    author: pr.user.as_ref().map(|u| u.login.clone()),
                    branch: Some(pr.head.r#ref),
                    base: Some(pr.base.r#ref),
                    domain: "code".into(),
                    language_hint: None,
                }
            }
            ReviewSource::DiffText {
                diff,
                title,
                domain,
                language_hint,
                description,
            } => ResolvedSource {
                pr_number: None,
                pr_title: Some(title),
                description,
                raw_diff: diff,
                file_contents: None,
                owner: None,
                repo: None,
                author: None,
                branch: None,
                base: None,
                domain,
                language_hint: Some(language_hint),
            },
            ReviewSource::Stdin {
                title,
                domain,
                language_hint,
            } => {
                let mut diff = String::new();
                tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::io::stdin().read_to_string(&mut diff),
                )
                .await
                .map_err(|_| AgentError::StdinTimeout)?
                .map_err(AgentError::Io)?;
                ResolvedSource {
                    pr_number: None,
                    pr_title: Some(title),
                    description: None,
                    raw_diff: diff,
                    file_contents: None,
                    owner: None,
                    repo: None,
                    author: None,
                    branch: None,
                    base: None,
                    domain,
                    language_hint: Some(language_hint),
                }
            }
            ReviewSource::File {
                path,
                domain,
                language,
                description,
            } => {
                let title = std::path::Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                let fc = file_reader::read_single(&path, language.as_deref()).map_err(|e| {
                    AgentError::Config(format!("Failed to read file '{path}': {e}"))
                })?;
                ResolvedSource {
                    pr_number: None,
                    pr_title: Some(title),
                    description,
                    raw_diff: String::new(),
                    file_contents: Some(vec![fc]),
                    owner: None,
                    repo: None,
                    author: None,
                    branch: None,
                    base: None,
                    domain,
                    language_hint: None,
                }
            }
            ReviewSource::Glob { pattern, domain } => {
                let files = file_reader::read_glob(&pattern).map_err(|e| {
                    AgentError::Config(format!("Failed to read glob '{pattern}': {e}"))
                })?;
                ResolvedSource {
                    pr_number: None,
                    pr_title: Some(format!("glob: {}", pattern)),
                    description: None,
                    raw_diff: String::new(),
                    file_contents: Some(files),
                    owner: None,
                    repo: None,
                    author: None,
                    branch: None,
                    base: None,
                    domain,
                    language_hint: None,
                }
            }
        };

        // ── 2. Content resolution + prompt building ────────────
        let pb = PromptBuilder::new(&resolved.domain);
        let system = pb.system_prompt();
        let system_tokens_estimated = pb.system_prompt_tokens();

        let make_ctx = || PromptContext {
            title: resolved.pr_title.as_deref().unwrap_or("(untitled)"),
            description: resolved.description.as_deref().unwrap_or(""),
            owner: resolved.owner.as_deref().unwrap_or(""),
            repo: resolved.repo.as_deref().unwrap_or(""),
            author: resolved.author.as_deref().unwrap_or(""),
            branch: resolved.branch.as_deref().unwrap_or(""),
            base: resolved.base.as_deref().unwrap_or(""),
            language_hint: resolved.language_hint.as_deref(),
        };

        let (
            files_changed,
            files_reviewed,
            files_skipped,
            files_path_filtered,
            files_budget_dropped,
            user,
            input_tokens_estimated,
        ) = if let Some(file_contents) = resolved.file_contents.as_ref() {
            Self::build_file_prompt(
                file_contents,
                &pb,
                &make_ctx(),
                &extra,
                &path_filter,
                self.diff_svc.max_tokens(),
            )
        } else {
            Self::build_diff_prompt(
                &resolved.raw_diff,
                &pb,
                &make_ctx(),
                &extra,
                &path_filter,
                &self.diff_svc,
            )
        };

        // ── 6. AI analysis ─────────────────────────────────────
        let finish_reason_len =
            |co: &crate::ai::ChatOutput| co.finish_reason.as_deref() == Some("length");
        let reported_tokens =
            |co: &crate::ai::ChatOutput| co.usage.as_ref().and_then(|u| u.completion_tokens);

        let mut chat_output = self.ai.chat(&system, &user).await?;
        let mut output_tokens_reported = reported_tokens(&chat_output);

        // ── 6b. Retry on length truncation (once, with halved tokens) ──
        if finish_reason_len(&chat_output)
            && output_tokens_reported.unwrap_or(u32::MAX) >= MIN_TOKENS_FOR_RETRY
        {
            let half = (self.ai.max_completion_tokens() / 2).max(512);
            tracing::warn!(
                finish_reason = ?chat_output.finish_reason,
                output_tokens = output_tokens_reported,
                half_max_tokens = half,
                "Response truncated — retrying with halved max_tokens"
            );
            chat_output = self.ai.chat_with_max_tokens(&system, &user, half).await?;
            output_tokens_reported = reported_tokens(&chat_output);
        }

        let sanitized = crate::services::sanitize_output(&chat_output.content);

        // ── 7. Extract structured findings ──────────────────────
        // The AI is instructed (via system prompt) to output a JSON
        // findings block before the markdown review.  Extraction is
        // best-effort: valid findings are preserved; the markdown
        // portion is always returned as `review_text`.
        let extracted = JsonExtractor::extract(&sanitized);
        let mut review_text = extracted.review_text;
        let mut findings = extracted.findings;
        let mut dropped_count = extracted.dropped_count;

        // ── 7b. Repair pass (single attempt) ────────────────────
        if dropped_count > 0 && !findings.is_empty() {
            // Some findings were usable but others had validation errors.
            // Attempt one repair round.
            let repair_msg = format!(
                "Your previous JSON had {dropped_count} malformed finding(s). \
                 Fix and output ONLY the corrected JSON array. \
                 Errors were: invalid severity, invalid category, missing \
                 required fields ('severity', 'category', 'message'), or \
                 empty 'message'."
            );
            tracing::warn!(dropped_count, "Attempting JSON repair pass");
            if let Ok(repair_output) = self
                .ai
                .chat_with_max_tokens(&system, &repair_msg, 2000)
                .await
            {
                let repair_extracted = JsonExtractor::extract(&repair_output.content);
                if !repair_extracted.findings.is_empty() {
                    findings = repair_extracted.findings;
                    dropped_count = repair_extracted.dropped_count;
                    review_text = repair_extracted.review_text;
                } else {
                    tracing::warn!("Repair pass produced no usable findings — keeping originals");
                }
            } else {
                tracing::warn!("Repair pass failed — keeping original findings");
            }
        }

        if dropped_count > 0 && findings.is_empty() {
            tracing::warn!(
                dropped_count,
                "All findings were invalid — falling back to text-only review"
            );
        }

        // ── 8. Post (optional, always Markdown) ─────────────────
        if post_to_github {
            if let (Some(owner), Some(repo)) = (resolved.owner.as_ref(), resolved.repo.as_ref()) {
                if let Some(number) = resolved.pr_number {
                    if let Some(ref github) = self.github_svc {
                        github
                            .post_review(owner, repo, number, &review_text)
                            .await?;
                    } else {
                        tracing::error!(
                            "post_to_github is true but GITHUB_TOKEN is not configured — \
                             review for PR #{} was NOT posted to GitHub. Set GITHUB_TOKEN env var.",
                            number,
                        );
                    }
                } else {
                    tracing::warn!(
                        "post_to_github is true but review source lacks a PR number — no review posted"
                    );
                }
            } else {
                tracing::warn!(
                    "post_to_github is true but review source is not a GitHub PR — no review posted"
                );
            }
        }

        let latency_ms = start.elapsed().as_millis() as u64;

        info!(
            pr_number = ?resolved.pr_number,
            files_changed,
            files_reviewed,
            input_tokens_estimated,
            latency_ms,
            "Review complete"
        );

        let total_tokens_used = output_tokens_reported
            .map(|t| system_tokens_estimated + input_tokens_estimated + t as usize);

        Ok(ReviewResult {
            review_text,
            findings,
            pr_number: resolved.pr_number,
            pr_title: resolved.pr_title,
            stats: ReviewStats {
                files_changed,
                files_reviewed,
                files_skipped,
                files_path_filtered,
                files_budget_dropped,
                input_tokens_estimated,
                system_tokens_estimated,
                output_tokens_reported,
                total_tokens_used,
                latency_ms,
                model: self.ai.model_name().to_string(),
                prompt_version: format!("{}/1", resolved.domain),
                domain: resolved.domain.clone(),
            },
        })
    }

    /// Build the user prompt from diff content, applying filters and budget.
    fn build_diff_prompt(
        raw_diff: &str,
        pb: &PromptBuilder,
        ctx: &PromptContext<'_>,
        extra: &str,
        path_filter: &[String],
        diff_svc: &DiffService,
    ) -> (usize, usize, usize, usize, usize, String, usize) {
        // Parse + filter
        let (files_changed, filtered) = diff_svc
            .parse_and_filter(raw_diff)
            .unwrap_or_else(|_| (0, vec![]));
        let files_skipped = files_changed.saturating_sub(filtered.len());

        // Path filter
        let pre_path = filtered.len();
        let after_path_filter: Vec<_> = if path_filter.is_empty() {
            filtered
        } else {
            filtered
                .into_iter()
                .filter(|f| {
                    path_filter
                        .iter()
                        .any(|p| !p.is_empty() && f.filename.starts_with(p))
                })
                .collect()
        };
        let files_path_filtered = pre_path.saturating_sub(after_path_filter.len());

        // Budget truncation
        let mut budgeted = after_path_filter;
        let system_overhead = estimate_tokens(include_str!("../prompts/code/system.txt"));
        let overhead = PROMPT_OVERHEAD_BASELINE
            + PROMPT_OVERHEAD_PER_FILE * budgeted.len()
            + system_overhead
            + if extra.is_empty() {
                0
            } else {
                estimate_tokens(extra) + 10
            };
        let effective_budget = diff_svc.max_tokens().saturating_sub(overhead);
        let files_budget_dropped = diff_svc.truncate_to_budget(&mut budgeted, effective_budget);

        let files_reviewed = budgeted.len();
        let user = pb.user_prompt(ctx, &budgeted, extra);
        let input_tokens_estimated = estimate_tokens(&user);

        (
            files_changed,
            files_reviewed,
            files_skipped,
            files_path_filtered,
            files_budget_dropped,
            user,
            input_tokens_estimated,
        )
    }

    /// Build the user prompt from file content, applying filters and budget.
    fn build_file_prompt(
        file_contents: &[FileContent],
        pb: &PromptBuilder,
        ctx: &PromptContext<'_>,
        extra: &str,
        path_filter: &[String],
        max_tokens: usize,
    ) -> (usize, usize, usize, usize, usize, String, usize) {
        let files_changed = file_contents.len();

        // Path filter on file paths
        let pre_path = file_contents.len();
        let after_path_filter: Vec<_> = if path_filter.is_empty() {
            file_contents.to_vec()
        } else {
            file_contents
                .iter()
                .filter(|f| {
                    path_filter
                        .iter()
                        .any(|p| !p.is_empty() && f.path.starts_with(p))
                })
                .cloned()
                .collect()
        };
        let files_skipped = 0; // No skip-list for file content
        let files_path_filtered = pre_path.saturating_sub(after_path_filter.len());

        // Budget truncation
        let mut budgeted = after_path_filter;
        let system_overhead = estimate_tokens(include_str!("../prompts/code/system.txt"));
        let overhead = PROMPT_OVERHEAD_BASELINE
            + PROMPT_OVERHEAD_PER_FILE * budgeted.len()
            + system_overhead
            + if extra.is_empty() {
                0
            } else {
                estimate_tokens(extra) + 10
            };
        let effective_budget = max_tokens.saturating_sub(overhead);
        let files_budget_dropped =
            file_reader::truncate_file_content_budget(&mut budgeted, effective_budget);

        let files_reviewed = budgeted.len();
        let user = pb.user_prompt_for_files(ctx, &budgeted, extra);
        let input_tokens_estimated = estimate_tokens(&user);

        (
            files_changed,
            files_reviewed,
            files_skipped,
            files_path_filtered,
            files_budget_dropped,
            user,
            input_tokens_estimated,
        )
    }
}
