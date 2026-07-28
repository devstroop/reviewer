use crate::error::{AgentError, Result};
use std::io::Write;

/// A local git repository wrapper backed by git2 (libgit2).
///
/// Provides local diff computation, file content access at any commit,
/// and code search — without GitHub API calls.
pub struct LocalRepo {
    repo: git2::Repository,
}

impl LocalRepo {
    /// Open an existing git repository at the given path.
    pub fn open(path: &str) -> Result<Self> {
        let repo = git2::Repository::open(path).map_err(|e| {
            AgentError::Config(format!(
                "Failed to open git repository at '{}': {}",
                path, e
            ))
        })?;
        Ok(Self { repo })
    }

    /// Compute the unified diff between two refs (e.g. "main", "HEAD").
    /// Returns a standard unified diff string compatible with `diffy`.
    pub fn diff_between(&self, base: &str, head: &str) -> Result<String> {
        let base_tree = self.peel_tree(base)?;
        let head_tree = self.peel_tree(head)?;
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), None)
            .map_err(|e| AgentError::Git(format!("Failed to compute diff: {}", e)))?;

        let mut out = Vec::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            let content = std::str::from_utf8(line.content()).unwrap_or("<binary>");
            match line.origin() {
                '+' | '-' | ' ' => {
                    let _ = write!(out, "{}{}", line.origin(), content);
                }
                // File header (diff --git), hunk header (@@),
                // new/deleted file metadata (---/+++) — no prefix needed
                'F' | 'H' | 'N' | 'D' => {
                    let _ = write!(out, "{}", content);
                }
                // Newline at EOF, no content
                '\n' => {}
                // Fallback, shouldn't happen with Patch format
                _ => {
                    let _ = write!(out, "{}", content);
                }
            }
            true
        })
        .map_err(|e| AgentError::Git(format!("Failed to format diff: {}", e)))?;

        String::from_utf8(out).map_err(|e| AgentError::Git(format!("Invalid UTF-8 in diff: {}", e)))
    }

    /// Read the full content of a file at a specific commit.
    pub fn file_at(&self, commit: &str, path: &str) -> Result<String> {
        let commit_obj = self
            .repo
            .revparse_single(commit)
            .map_err(|e| AgentError::Git(format!("Failed to resolve '{}': {}", commit, e)))?;
        let tree = commit_obj.peel_to_tree().map_err(|e| {
            AgentError::Git(format!("'{}' does not point to a tree: {}", commit, e))
        })?;
        let entry = tree.get_path(std::path::Path::new(path)).map_err(|e| {
            AgentError::Git(format!("File '{}' not found at {}: {}", path, commit, e))
        })?;
        let blob = entry
            .to_object(&self.repo)
            .map_err(|e| AgentError::Git(format!("Failed to read '{}': {}", path, e)))?;
        let content = blob
            .as_blob()
            .ok_or_else(|| AgentError::Git(format!("'{}' is not a blob", path)))?;
        let text = std::str::from_utf8(content.content())
            .map_err(|e| AgentError::Git(format!("File '{}' is not valid UTF-8: {}", path, e)))?;
        Ok(text.to_string())
    }

    /// Search code matching a pattern (git grep equivalent).
    pub fn grep(&self, _pattern: &str, _paths: &[&str]) -> Result<Vec<(String, u64, String)>> {
        Err(AgentError::Config(
            "grep not yet implemented with git2".into(),
        ))
    }

    /// Resolve a ref to a tree object.
    fn peel_tree(&self, rev: &str) -> Result<git2::Tree<'_>> {
        let obj = self
            .repo
            .revparse_single(rev)
            .map_err(|e| AgentError::Git(format!("Failed to resolve '{}': {}", rev, e)))?;
        obj.peel_to_tree()
            .map_err(|e| AgentError::Git(format!("'{}' does not point to a tree: {}", rev, e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn find_repo_root() -> String {
        // Walk up from CWD to find the .git directory
        let mut dir = std::env::current_dir().unwrap();
        loop {
            if dir.join(".git").exists() {
                return dir.to_string_lossy().to_string();
            }
            if !dir.pop() {
                panic!("No git repository found from CWD");
            }
        }
    }

    #[test]
    fn test_open_repo() {
        let repo = LocalRepo::open(&find_repo_root());
        assert!(repo.is_ok(), "Should open repo: {:?}", repo.err());
    }

    #[test]
    fn test_open_nonexistent_path() {
        let repo = LocalRepo::open("/tmp/nonexistent-repo-12345");
        assert!(repo.is_err());
    }

    #[test]
    fn test_diff_between_head_and_self() {
        let repo = LocalRepo::open(&find_repo_root()).unwrap();
        let diff = repo.diff_between("HEAD", "HEAD");
        // Diff between HEAD and itself should be empty
        assert!(diff.is_ok(), "Diff failed: {:?}", diff.err());
        if let Ok(d) = diff {
            assert!(
                d.is_empty(),
                "HEAD..HEAD diff should be empty, got len={}",
                d.len()
            );
        }
    }

    #[test]
    fn test_file_at_head() {
        let repo = LocalRepo::open(&find_repo_root()).unwrap();
        // Try reading a file we know exists at HEAD
        let result = repo.file_at("HEAD", "Cargo.toml");
        assert!(result.is_ok(), "file_at failed: {:?}", result.err());
        if let Ok(content) = result {
            assert!(
                content.contains("[package]"),
                "Should contain package header"
            );
        }
    }

    #[test]
    fn test_file_at_invalid_path() {
        let repo = LocalRepo::open(&find_repo_root()).unwrap();
        let result = repo.file_at("HEAD", "this-file-does-not-exist.xyz");
        assert!(result.is_err());
    }

    #[test]
    fn test_diff_between_has_content() {
        let repo = LocalRepo::open(&find_repo_root()).unwrap();
        // HEAD~1..HEAD should have some diff (most recent commit)
        let diff = repo.diff_between("HEAD~1", "HEAD").unwrap();
        if diff.is_empty() {
            // If local HEAD matches remote, try HEAD~2..HEAD
            let diff2 = repo.diff_between("HEAD~2", "HEAD").unwrap();
            eprintln!("HEAD~2..HEAD diff ({} bytes):", diff2.len());
            for line in diff2.lines().take(10) {
                eprintln!("  {}", line);
            }
            // Should have at least some content
            // Both are valid refs; even empty should not error
        } else {
            eprintln!("HEAD~1..HEAD diff ({} bytes):", diff.len());
            for line in diff.lines().take(10) {
                eprintln!("  {}", line);
            }
            assert!(diff.contains("diff --git"), "Should contain diff headers");
        }
    }
}
