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
use std::collections::HashSet;
use std::path::Path;

/// Maximum estimated tokens for injected rule text.
/// If combined rules exceed this, excess files are dropped with a warning.
const MAX_RULES_TOKENS: usize = 2000;

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

/// A single rule entry from a rules.json index.
#[derive(Debug, Clone, serde::Deserialize)]
struct RuleEntry {
    /// Glob pattern like "**/*.rs" or "**/*.{ts,js}"
    pattern: String,
    /// Rule file names to apply (without path, relative to rules dir)
    files: Vec<String>,
}

/// The rule index loaded from a rules.json file (which is a top-level array).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(transparent)]
struct RuleIndex {
    rules: Vec<RuleEntry>,
}

impl RuleIndex {
    /// Load rules from a rules.json file at the given path.
    fn load_from(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }
}

/// Resolved rule content: loaded rule text ready for prompt injection.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedRules {
    /// Concatenated rule text (all matched rules joined with section headers).
    pub text: String,
    /// Number of unique rule files matched.
    #[allow(dead_code)]
    pub count: usize,
}

/// Builds the system and user prompts for the AI, per domain.
pub(crate) struct PromptBuilder {
    /// Review domain (e.g. "code", "config", "policy").
    domain: String,
    /// Cached system prompt for this domain.
    system_prompt: String,
    /// Cached user template for this domain.
    user_template: String,
    /// Loaded rule index (built-in + project-level merged).
    rule_index: Option<RuleIndex>,
    /// Cached rule file contents mapped by filename.
    rule_contents: std::collections::HashMap<String, String>,
}

impl PromptBuilder {
    /// Create a new PromptBuilder for the given domain.
    ///
    /// Loads prompt files from `prompts/{domain}/` at runtime.
    /// Falls back through: `{domain}` → `code` → compiled default.
    /// Also loads rules from `prompts/{domain}/rules/rules.json`
    /// and optionally from `.reviewer/rules.json` in CWD.
    pub(crate) fn new(domain: &str) -> Self {
        let system_prompt = Self::load_prompt(domain, "system.txt").unwrap_or_else(|| {
            Self::load_prompt("code", "system.txt")
                .unwrap_or_else(|| CODE_SYSTEM_FALLBACK.to_string())
        });
        let user_template = Self::load_prompt(domain, "user.txt").unwrap_or_else(|| {
            Self::load_prompt("code", "user.txt").unwrap_or_else(|| CODE_USER_FALLBACK.to_string())
        });

        let (rule_index, rule_contents) = Self::load_rules(domain);

        Self {
            domain: domain.to_string(),
            system_prompt,
            user_template,
            rule_index,
            rule_contents,
        }
    }

    /// Load rules from all sources (built-in + project-level) and merge them.
    /// Project-level rules extend (not replace) built-in rules.
    fn load_rules(domain: &str) -> (Option<RuleIndex>, std::collections::HashMap<String, String>) {
        let mut contents: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut all_entries: Vec<RuleEntry> = Vec::new();

        // 1. Built-in rules: prompts/{domain}/rules/rules.json
        let rules_dir = Path::new("prompts").join(domain).join("rules");
        if let Some(index) = Self::load_rule_index(&rules_dir) {
            for entry in &index.rules {
                for file in &entry.files {
                    if !contents.contains_key(file) && Self::is_safe_rule_path(file) {
                        let path = rules_dir.join(file);
                        if let Ok(text) = std::fs::read_to_string(&path) {
                            contents.insert(file.clone(), text);
                        }
                    }
                }
            }
            all_entries.extend(index.rules);
        }

        // 2. Project-level rules: .reviewer/rules.json (extends built-in)
        let project_rules_path = Path::new(".reviewer").join("rules.json");
        let project_rules_dir = Path::new(".reviewer");
        if project_rules_path.exists() {
            if let Some(index) = Self::load_rule_index(project_rules_dir) {
                for entry in &index.rules {
                    for file in &entry.files {
                        if !Self::is_safe_rule_path(file) {
                            continue;
                        }
                        if contents.contains_key(file) {
                            tracing::warn!(
                                "Project-level rule '{}' has same name as built-in rule — \
                                 keeping built-in (extend-only; rename to override)",
                                file
                            );
                            continue;
                        }
                        let path = project_rules_dir.join(file);
                        if let Ok(text) = std::fs::read_to_string(&path) {
                            contents.insert(file.clone(), text);
                        }
                    }
                }
                all_entries.extend(index.rules);
            }
        }

        let index = if all_entries.is_empty() {
            None
        } else {
            Some(RuleIndex { rules: all_entries })
        };

        (index, contents)
    }

    /// Load a rule index from a directory containing rules.json.
    fn load_rule_index(dir: &Path) -> Option<RuleIndex> {
        let path = dir.join("rules.json");
        RuleIndex::load_from(&path)
    }

    /// Validate that a rule file path is safe: must be a plain filename
    /// with an allowed extension, no path separators, no `..`.
    fn is_safe_rule_path(file: &str) -> bool {
        let allowed_ext = [".md", ".txt", ".markdown"];
        let p = Path::new(file);
        // Reject paths with directory components
        if p.parent().map(|p| p != Path::new("")).unwrap_or(false) {
            return false;
        }
        // Must have an allowed extension
        p.extension()
            .map(|e| {
                let ext = format!(".{}", e.to_string_lossy().to_lowercase());
                allowed_ext.contains(&ext.as_str())
            })
            .unwrap_or(false)
    }

    /// Try to load a prompt file from `prompts/{domain}/{file}`.
    /// Domain is validated to prevent path traversal: only alphanumeric, `-`, `_`.
    fn load_prompt(domain: &str, file: &str) -> Option<String> {
        if domain.is_empty()
            || domain.contains('/')
            || domain.contains('\\')
            || domain.contains("..")
            || !domain
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        let path = Path::new("prompts").join(domain).join(file);
        std::fs::read_to_string(&path).ok()
    }

    /// Resolve rules for a set of files.
    ///
    /// Returns the concatenated rule text and match count.
    /// The returned text can be injected into `{system_rule}` in the system prompt.
    pub(crate) fn resolve_rules(&self, files: &[DiffFile]) -> ResolvedRules {
        let Some(ref index) = self.rule_index else {
            return ResolvedRules {
                text: String::new(),
                count: 0,
            };
        };

        let mut matched_files: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for entry in &index.rules {
            let matched = files
                .iter()
                .any(|f| glob_match_basic(&entry.pattern, &f.filename));
            if matched {
                for file in &entry.files {
                    if seen.insert(file) {
                        matched_files.push(file);
                    }
                }
            }
        }

        if matched_files.is_empty() {
            return ResolvedRules {
                text: String::new(),
                count: 0,
            };
        }

        let mut text = String::new();
        let mut truncated = false;
        for file in &matched_files {
            if let Some(content) = self.rule_contents.get(*file) {
                let section = format!("### {}\n\n{}\n", file.trim_end_matches(".md"), content);
                if estimate_tokens(&text) + estimate_tokens(&section) > MAX_RULES_TOKENS {
                    truncated = true;
                    break;
                }
                text.push_str(&section);
            }
        }

        if truncated {
            tracing::warn!(
                "Rule text truncated at {} estimated tokens ({} matched files)",
                MAX_RULES_TOKENS,
                matched_files.len()
            );
        }

        ResolvedRules {
            text,
            count: matched_files.len(),
        }
    }

    /// Return the system prompt with `{system_rule}` replaced by the given rules text.
    /// If rules_text is empty, the placeholder is removed entirely.
    pub(crate) fn system_prompt(&self, rules_text: &str) -> String {
        self.system_prompt.replace("{system_rule}", rules_text)
    }

    /// Return the estimated token count of the system prompt with rules injected.
    pub(crate) fn system_prompt_tokens(&self, rules_text: &str) -> usize {
        estimate_tokens(&self.system_prompt(rules_text))
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
        let content_format = format!("### Diff\n\n```\n{}\n```", diff_context);
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
            if detected != "Unknown" {
                detected
            } else {
                "Unknown".to_string()
            }
        };
        let file_list = self.format_file_list_from_files(files);
        let file_context = format_file_context(files);
        let total_files = files.len();

        let pr_metadata = self.pr_metadata_section(ctx, &language);
        let content_format = format!("### File Contents\n\n{}", file_context);
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
            out.push_str(&format!(
                "- `{}` ({} — {} lines)\n",
                f.path, f.language, f.line_count
            ));
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

/// Basic glob pattern matcher for the subset of patterns we use.
///
/// Supports:
/// - `**/*.ext` — matches files ending with `.ext`
/// - `**/*.{ext1,ext2}` — matches files ending with any of the given extensions
/// - `**/filename` — matches files named exactly `filename`
/// - `**/*.{ts,tsx,js,jsx}` — combined multi-extension patterns
///
/// Matching is case-insensitive (both pattern and filename are lowercased).
fn glob_match_basic(pattern: &str, filename: &str) -> bool {
    let filename_lower = filename.to_lowercase();
    let pattern_lower = pattern.to_lowercase();

    // Normalize: remove leading **/
    let p = if let Some(rest) = pattern_lower.strip_prefix("**/") {
        rest
    } else {
        &pattern_lower
    };

    // Handle {ext1,ext2,...} in the last path segment
    if let Some(open) = p.find('{') {
        if let Some(close) = p.find('}') {
            let prefix = &p[..open];
            let alternatives = &p[open + 1..close];
            if let Some(rest) = p.get(close + 1..) {
                for alt in alternatives.split(',') {
                    let expanded = format!("{}{}{}", prefix, alt.trim(), rest);
                    if glob_match_basic(&expanded, filename) {
                        return true;
                    }
                }
            }
            return false;
        }
    }

    // Handle *.ext patterns
    if let Some(ext) = p.strip_prefix('*') {
        if filename_lower.ends_with(ext) {
            return true;
        }
        return false;
    }

    // Exact filename match (e.g. "Dockerfile")
    if !p.contains('*') {
        let basename = std::path::Path::new(filename)
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or(std::borrow::Cow::Borrowed(filename));
        return basename.to_lowercase() == p;
    }

    // Fallback: check extension if pattern is like "*.ext" without leading *
    if let Some(ext) = p.strip_prefix('.') {
        if filename_lower.ends_with(ext) || format!(".{}", ext) == p {
            return true;
        }
    }

    false
}

/// Format file content for inclusion in the AI prompt.
pub(crate) fn format_file_context(files: &[FileContent]) -> String {
    let mut out = String::new();
    for f in files {
        out.push_str(&format!("### File: {} ({})\n\n", f.path, f.language));
        out.push_str(&format!("```{}\n", f.language.to_lowercase()));
        let escaped = f.content.replace("```", "\\`\\`\\`");
        out.push_str(&escaped);
        if !escaped.ends_with('\n') {
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

    #[test]
    fn glob_match_extension() {
        assert!(glob_match_basic("**/*.rs", "src/main.rs"));
        assert!(glob_match_basic("**/*.rs", "main.rs"));
        assert!(!glob_match_basic("**/*.rs", "main.py"));
        assert!(!glob_match_basic("**/*.rs", "rs"));
    }

    #[test]
    fn glob_match_basename() {
        assert!(glob_match_basic("**/Dockerfile", "Dockerfile"));
        assert!(glob_match_basic("**/Dockerfile", "path/to/Dockerfile"));
        assert!(!glob_match_basic("**/Dockerfile", "Dockerfile.prod"));
    }

    #[test]
    fn glob_match_brace_expansion() {
        assert!(glob_match_basic("**/*.{ts,tsx,js,jsx}", "app.ts"));
        assert!(glob_match_basic("**/*.{ts,tsx,js,jsx}", "app.tsx"));
        assert!(glob_match_basic("**/*.{ts,tsx,js,jsx}", "app.js"));
        assert!(glob_match_basic("**/*.{ts,tsx,js,jsx}", "app.jsx"));
        assert!(!glob_match_basic("**/*.{ts,tsx,js,jsx}", "app.py"));
        assert!(glob_match_basic("**/*.{c,h}", "file.c"));
        assert!(glob_match_basic("**/*.{c,h}", "file.h"));
    }

    #[test]
    fn glob_match_case_insensitive() {
        // Both sides are lowercased — `.Rs` matches `.rs`
        assert!(glob_match_basic("**/*.rs", "Main.Rs"));
        assert!(glob_match_basic("**/*.Rs", "main.rs"));
    }

    #[test]
    fn is_safe_rule_path_accepts_md() {
        assert!(PromptBuilder::is_safe_rule_path("rust.md"));
        assert!(PromptBuilder::is_safe_rule_path("security.txt"));
        assert!(PromptBuilder::is_safe_rule_path("rules.markdown"));
    }

    #[test]
    fn is_safe_rule_path_rejects_traversal() {
        assert!(!PromptBuilder::is_safe_rule_path("../etc/passwd"));
        assert!(!PromptBuilder::is_safe_rule_path("subdir/../file.md"));
        assert!(!PromptBuilder::is_safe_rule_path("/absolute/path.md"));
    }

    #[test]
    fn is_safe_rule_path_rejects_bad_ext() {
        assert!(!PromptBuilder::is_safe_rule_path("rules.json"));
        assert!(!PromptBuilder::is_safe_rule_path("script.sh"));
        assert!(!PromptBuilder::is_safe_rule_path("rust.md.exe"));
    }

    #[test]
    fn rule_index_deserializes_from_array() {
        let json = r#"[
            {"pattern": "**/*.rs", "files": ["rust.md"]},
            {"pattern": "**/*.py", "files": ["python.md"]}
        ]"#;
        let index: RuleIndex =
            serde_json::from_str(json).expect("RuleIndex deserialization failed");
        assert_eq!(index.rules.len(), 2);
        assert_eq!(index.rules[0].pattern, "**/*.rs");
        assert_eq!(index.rules[1].files[0], "python.md");
    }

    #[test]
    fn rule_index_empty_array_is_valid() {
        let json = r#"[]"#;
        let index: RuleIndex =
            serde_json::from_str(json).expect("Empty RuleIndex should deserialize");
        assert!(index.rules.is_empty());
    }
}
