//! Fills the system and user prompt templates with context about
//! the PR and diff.  Only `ReviewEngine` calls this service.
//!
//! Prompts are loaded at runtime from `prompts/{domain}/` directory.
//! Fallback chain: `{domain}/system.txt` → `code/system.txt` → compiled default.
//! This allows adding new domains by just writing prompt files — no code changes.

use crate::diff::{DiffFile, DiffStatus, format_diff_context};
use crate::language::detect_language;
use crate::services::file_reader::FileContent;
use crate::tokens::estimate_tokens;
use std::path::Path;

/// Compiled-in fallback for the `code` domain system prompt.
/// Used when the prompt file is not available on the filesystem.
const CODE_SYSTEM_FALLBACK: &str = include_str!("../../prompts/code/system.txt");

/// Compiled-in fallback for the `code` domain user template.
const CODE_USER_FALLBACK: &str = include_str!("../../prompts/code/user.txt");

/// All PR metadata fields used to fill the user prompt template.
pub(crate) struct PromptContext<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
    pub author: &'a str,
    pub branch: &'a str,
    pub base: &'a str,
    /// Optional language hint from the caller (DiffText source).
    /// Used as a fallback when file-extension detection yields "Unknown".
    pub language_hint: Option<&'a str>,
}

/// Builds the system and user prompts for the AI, per domain.
pub(crate) struct PromptBuilder {
    /// Review domain (e.g. "code", "config", "policy").
    domain: String,
    /// Cached system prompt for this domain.
    system_prompt: String,
    /// Cached user template for this domain.
    user_template: String,
}

impl PromptBuilder {
    /// Create a new PromptBuilder for the given domain.
    ///
    /// Loads prompt files from `prompts/{domain}/` at runtime.
    /// Falls back through: `{domain}` → `code` → compiled default.
    pub(crate) fn new(domain: &str) -> Self {
        let system_prompt = Self::load_prompt(domain, "system.txt")
            .unwrap_or_else(|| Self::load_prompt("code", "system.txt")
                .unwrap_or_else(|| CODE_SYSTEM_FALLBACK.to_string()));
        let user_template = Self::load_prompt(domain, "user.txt")
            .unwrap_or_else(|| Self::load_prompt("code", "user.txt")
                .unwrap_or_else(|| CODE_USER_FALLBACK.to_string()));
        Self {
            domain: domain.to_string(),
            system_prompt,
            user_template,
        }
    }

    /// Try to load a prompt file from `prompts/{domain}/{file}`.
    fn load_prompt(domain: &str, file: &str) -> Option<String> {
        let path = Path::new("prompts").join(domain).join(file);
        std::fs::read_to_string(&path).ok()
    }

    /// Return the system prompt for the current domain.
    pub(crate) fn system_prompt(&self) -> String {
        self.system_prompt.clone()
    }

    /// Return the estimated token count of the system prompt.
    pub(crate) fn system_prompt_tokens(&self) -> usize {
        estimate_tokens(&self.system_prompt)
    }

    /// Fill the user prompt template with metadata and diff context.
    pub(crate) fn user_prompt(
        &self,
        ctx: &PromptContext<'_>,
        files: &[DiffFile],
        extra: &str,
    ) -> String {
        let language = self.detect_language(ctx, files);
        let file_list = self.format_file_list(files);
        let diff_context = format_diff_context(files, usize::MAX);
        let total_files = files.len();

        let pr_metadata = self.pr_metadata_section(ctx, &language);
        let content_format = format!(
            "### Diff\n\n```\n{}\n```",
            diff_context
        );
        let extra_section = if extra.is_empty() {
            String::new()
        } else {
            format!("### Additional Instructions\n\n{}\n", extra)
        };

        self.user_template
            .replace("{title}", ctx.title)
            .replace("{pr_metadata}", &pr_metadata)
            .replace("{owner}", ctx.owner)
            .replace("{repo}", ctx.repo)
            .replace("{author}", ctx.author)
            .replace("{branch}", ctx.branch)
            .replace("{base}", ctx.base)
            .replace("{language}", &language)
            .replace("{description}", ctx.description)
            .replace("{total_files}", &total_files.to_string())
            .replace("{file_list}", &file_list)
            .replace("{extra_instructions}", &extra_section)
            .replace("{content_format}", &content_format)
    }

    /// Fill the user prompt template with metadata and FILE content (not diff).
    pub(crate) fn user_prompt_for_files(
        &self,
        ctx: &PromptContext<'_>,
        files: &[FileContent],
        extra: &str,
    ) -> String {
        let language = if let Some(hint) = ctx.language_hint {
            hint.to_string()
        } else {
            let detected = Self::detect_primary_from_files(files);
            if detected != "Unknown" { detected } else { "Unknown".to_string() }
        };
        let file_list = self.format_file_list_from_files(files);
        let file_context = format_file_context(files);
        let total_files = files.len();

        let pr_metadata = self.pr_metadata_section(ctx, &language);
        let content_format = format!(
            "### File Contents\n\n{}",
            file_context
        );
        let extra_section = if extra.is_empty() {
            String::new()
        } else {
            format!("### Additional Instructions\n\n{}\n", extra)
        };

        self.user_template
            .replace("{title}", ctx.title)
            .replace("{pr_metadata}", &pr_metadata)
            .replace("{owner}", ctx.owner)
            .replace("{repo}", ctx.repo)
            .replace("{author}", ctx.author)
            .replace("{branch}", ctx.branch)
            .replace("{base}", ctx.base)
            .replace("{language}", &language)
            .replace("{description}", ctx.description)
            .replace("{total_files}", &total_files.to_string())
            .replace("{file_list}", &file_list)
            .replace("{extra_instructions}", &extra_section)
            .replace("{content_format}", &content_format)
    }

    /// Build the PR metadata section. Returns empty string when owner is absent
    /// (file/glob sources).
    fn pr_metadata_section(&self, ctx: &PromptContext<'_>, language: &str) -> String {
        if ctx.owner.is_empty() {
            return format!(
                "## {}\n\n**Language:** {}\n**Description:** {}",
                ctx.title, language, ctx.description
            );
        }
        format!(
            "## PR: {title}\n\n**Repository:** {owner}/{repo}\n**Author:** {author}\n**Branch:** {branch} → {base}\n**Language:** {language}\n**Description:** {description}",
            title = ctx.title,
            owner = ctx.owner,
            repo = ctx.repo,
            author = ctx.author,
            branch = ctx.branch,
            base = ctx.base,
            language = language,
            description = ctx.description,
        )
    }

    /// Detect the primary language, or return language hint.
    /// Only runs language detection for the "code" domain.
    fn detect_language(&self, ctx: &PromptContext<'_>, files: &[DiffFile]) -> String {
        if self.domain == "code" {
            let detected = self.detect_primary_language(files);
            if detected != "Unknown" {
                return detected;
            }
            if let Some(hint) = ctx.language_hint {
                return hint.to_string();
            }
            "Unknown".to_string()
        } else {
            // Non-code domains don't need language detection.
            String::new()
        }
    }

    /// Determine the primary language from the changed files.
    fn detect_primary_language(&self, files: &[DiffFile]) -> String {
        use std::collections::HashMap;

        let mut counts: HashMap<String, usize> = HashMap::new();
        for f in files {
            let lang = detect_language(&f.filename).to_string();
            *counts.entry(lang).or_insert(0) += 1;
        }

        counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .map(|(lang, _)| lang)
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Format a simple bullet list of changed files with their status.
    fn format_file_list(&self, files: &[DiffFile]) -> String {
        let mut out = String::new();
        for f in files {
            let status = match f.status {
                DiffStatus::Added => "added",
                DiffStatus::Modified => "modified",
                DiffStatus::Deleted => "deleted",
                DiffStatus::Renamed => "renamed",
                DiffStatus::Copied => "copied",
            };
            out.push_str(&format!(
                "- `{}` ({} — +{} −{})\n",
                f.filename, status, f.additions, f.deletions
            ));
        }
        if out.is_empty() {
            out.push_str("- (no reviewable files)");
        }
        out
    }

    /// Format a bullet list of files with their language (for file content mode).
    fn format_file_list_from_files(&self, files: &[FileContent]) -> String {
        let mut out = String::new();
        for f in files {
            out.push_str(&format!("- `{}` ({} — {} lines)\n", f.path, f.language, f.line_count));
        }
        if out.is_empty() {
            out.push_str("- (no files)");
        }
        out
    }

    /// Determine the primary language from FileContent entries.
    fn detect_primary_from_files(files: &[FileContent]) -> String {
        use std::collections::HashMap;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for f in files {
            *counts.entry(f.language.clone()).or_insert(0) += 1;
        }
        counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .map(|(lang, _)| lang)
            .unwrap_or_else(|| "Unknown".to_string())
    }
}

/// Format file content for inclusion in the AI prompt.
pub(crate) fn format_file_context(files: &[FileContent]) -> String {
    let mut out = String::new();
    for f in files {
        out.push_str(&format!("### File: {} ({})\n\n", f.path, f.language));
        out.push_str(&format!("```{}\n", f.language.to_lowercase()));
        out.push_str(&f.content);
        if !f.content.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffFile, DiffStatus, Hunk};

    fn make_file(name: &str, status: DiffStatus) -> DiffFile {
        DiffFile {
            filename: name.to_string(),
            old_filename: None,
            status,
            hunks: vec![Hunk {
                header: "@@ -1 +1 @@".into(),
                lines: vec![],
            }],
            additions: 5,
            deletions: 3,
            mode_change: None,
        }
    }

    #[test]
    fn detect_primary_language_finds_most_common() {
        let builder = PromptBuilder::new("code");
        let files = vec![
            make_file("a.rs", DiffStatus::Modified),
            make_file("b.py", DiffStatus::Added),
            make_file("c.rs", DiffStatus::Modified),
            make_file("d.js", DiffStatus::Added),
        ];
        let result = builder.detect_primary_language(&files);
        assert_eq!(result, "Rust");
    }

    #[test]
    fn detect_primary_language_empty_returns_unknown() {
        let builder = PromptBuilder::new("code");
        assert_eq!(builder.detect_primary_language(&[]), "Unknown");
    }

    #[test]
    fn detect_primary_language_tie_returns_first_max() {
        let builder = PromptBuilder::new("code");
        let files = vec![
            make_file("a.rs", DiffStatus::Modified),
            make_file("b.py", DiffStatus::Modified),
        ];
        let result = builder.detect_primary_language(&files);
        assert!(result == "Rust" || result == "Python", "got {result}");
    }

    #[test]
    fn format_file_list_basic() {
        let builder = PromptBuilder::new("code");
        let files = vec![
            make_file("src/main.rs", DiffStatus::Modified),
            make_file("src/lib.rs", DiffStatus::Added),
        ];
        let result = builder.format_file_list(&files);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains("+5"));
        assert!(result.contains("−3"));
        assert!(result.contains("modified"));
        assert!(result.contains("added"));
    }

    #[test]
    fn format_file_list_empty() {
        let builder = PromptBuilder::new("code");
        assert_eq!(builder.format_file_list(&[]), "- (no reviewable files)");
    }

}
