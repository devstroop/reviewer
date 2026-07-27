use crate::error::Result;
use crate::language::detect_language;
use crate::tokens::estimate_tokens;
use std::path::Path;
use tracing::warn;

/// A source file read from the filesystem, ready for review.
#[derive(Debug, Clone)]
pub struct FileContent {
    /// Path relative to CWD or as given by the caller.
    pub path: String,
    /// Full text content of the file.
    pub content: String,
    /// Detected programming language (from file extension).
    pub language: String,
    /// Number of lines in the file.
    pub line_count: u64,
}

/// Overhead per file in the prompt when rendering file content.
const FILE_OVERHEAD_CHARS: usize = 60;

/// Read a single file from the filesystem.
pub(crate) fn read_single(path: &str, language_override: Option<&str>) -> Result<FileContent> {
    let p = Path::new(path);
    validate_path(p)?;

    let metadata = p
        .metadata()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{path}: {e}")))?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{path}: not a regular file"),
        )
        .into());
    }
    if metadata.len() > 1_048_576 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{path}: file exceeds 1 MB limit ({} bytes)", metadata.len()),
        )
        .into());
    }

    let content =
        std::fs::read_to_string(p).map_err(|e| std::io::Error::other(format!("{path}: {e}")))?;

    if is_binary(content.as_bytes()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{path}: binary file not supported"),
        )
        .into());
    }

    let language = language_override
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| detect_language(path).to_string());
    let line_count = content.lines().count() as u64;

    Ok(FileContent {
        path: path.to_string(),
        content,
        language,
        line_count,
    })
}

/// Read all files matching a glob pattern.
pub(crate) fn read_glob(pattern: &str) -> Result<Vec<FileContent>> {
    let mut files: Vec<FileContent> = Vec::new();
    let entries = glob::glob(pattern).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid glob pattern '{pattern}': {e}"),
        )
    })?;

    for entry in entries {
        let entry_path = entry.map_err(|e| std::io::Error::other(format!("glob error: {e}")))?;
        let path_str = entry_path.to_string_lossy().to_string();
        match read_single(&path_str, None) {
            Ok(fc) => files.push(fc),
            Err(e) => warn!("Skipping {}: {}", path_str, e),
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Drop files (largest content first) until total fits within `budget` tokens.
/// Returns the number of files dropped. Keeps at least one file.
pub(crate) fn truncate_file_content_budget(files: &mut Vec<FileContent>, budget: usize) -> usize {
    if files.is_empty() {
        return 0;
    }

    let mut indexed: Vec<(usize, usize)> = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let tokens = estimate_tokens(&f.content) + FILE_OVERHEAD_CHARS * 2 / 7;
            (i, tokens)
        })
        .collect();

    let total: usize = indexed.iter().map(|(_, t)| *t).sum();
    if total <= budget {
        return 0;
    }

    indexed.sort_by_key(|(_, t)| std::cmp::Reverse(*t));

    let mut to_drop: Vec<usize> = Vec::new();
    let mut running_total = total;

    for (i, (idx, tokens)) in indexed.into_iter().enumerate() {
        if running_total <= budget {
            break;
        }
        if i == files.len() - 1 {
            break;
        }
        to_drop.push(idx);
        running_total -= tokens;
        warn!(tokens, "Dropping file — exceeded token budget ({})", budget);
    }

    to_drop.sort_unstable_by(|a, b| b.cmp(a));
    let dropped = to_drop.len();
    for idx in to_drop {
        files.remove(idx);
    }

    dropped
}

/// Check first 8KB for null bytes to detect binary files.
fn is_binary(bytes: &[u8]) -> bool {
    let check_len = bytes.len().min(8192);
    bytes[..check_len].contains(&0)
}

/// Validate a file path: reject `..`, reject symlinks.
fn validate_path(p: &Path) -> Result<()> {
    let path_str = p.to_string_lossy();
    if path_str.contains("..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path traversal rejected: '{path_str}' contains '..'"),
        )
        .into());
    }

    #[cfg(unix)]
    {
        if let Ok(meta) = p.symlink_metadata() {
            if meta.file_type().is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("symlink rejected: '{path_str}'"),
                )
                .into());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_single_valid_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "fn main() {{}}\n").unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let fc = read_single(&path, None).unwrap();
        assert_eq!(fc.content, "fn main() {}\n");
        assert_eq!(fc.line_count, 1);
        assert_eq!(fc.language, "Unknown"); // .tmp extension
    }

    #[test]
    fn test_read_single_with_language_override() {
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "hello").unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        let fc = read_single(&path, Some("Rust")).unwrap();
        assert_eq!(fc.language, "Rust");
    }

    #[test]
    fn test_read_single_nonexistent() {
        assert!(read_single("/nonexistent-file-12345", None).is_err());
    }

    #[test]
    fn test_read_single_path_traversal_rejected() {
        assert!(read_single("../../etc/passwd", None).is_err());
    }

    #[test]
    fn test_read_single_binary_rejected() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&[0, 1, 2, 3]).unwrap();
        let path = tmp.path().to_string_lossy().to_string();
        assert!(read_single(&path, None).is_err());
    }

    #[test]
    fn test_truncate_file_content_budget_keeps_one() {
        let mut files = vec![
            FileContent {
                path: "a.rs".into(),
                content: "x".repeat(100_000),
                language: "Rust".into(),
                line_count: 5000,
            },
            FileContent {
                path: "b.rs".into(),
                content: "x".repeat(100_000),
                language: "Rust".into(),
                line_count: 5000,
            },
        ];
        let dropped = truncate_file_content_budget(&mut files, 100);
        assert_eq!(dropped, 1);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_truncate_file_content_budget_fits_all() {
        let mut files = vec![FileContent {
            path: "a.rs".into(),
            content: "small".into(),
            language: "Rust".into(),
            line_count: 1,
        }];
        let dropped = truncate_file_content_budget(&mut files, 1_000_000);
        assert_eq!(dropped, 0);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_is_binary_detects_null_bytes() {
        assert!(is_binary(&[0, 1, 2]));
        assert!(!is_binary(&[1, 2, 3]));
    }

    #[test]
    fn test_validate_path_rejects_dotdot() {
        assert!(validate_path(Path::new("../../foo")).is_err());
        assert!(validate_path(Path::new("foo/../bar")).is_err());
    }

    #[test]
    fn test_validate_path_accepts_normal() {
        assert!(validate_path(Path::new("src/main.rs")).is_ok());
        assert!(validate_path(Path::new("/absolute/path.rs")).is_ok());
    }
}
