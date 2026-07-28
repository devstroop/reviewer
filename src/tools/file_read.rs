use crate::tools::Tool;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

/// Read a file from the filesystem.
/// Args: `path` (required), `start_line` (optional), `end_line` (optional)
pub struct FileRead;

#[async_trait]
impl Tool for FileRead {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the content of a file from the filesystem. Specify path (relative to repo root). Optionally limit with start_line and end_line (1-based)."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file (relative to repo root)" },
                "start_line": { "type": "integer", "description": "First line to read (1-based, optional)" },
                "end_line": { "type": "integer", "description": "Last line to read (1-based, optional)" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing 'path' argument".to_string())?;

        let content = tokio::fs::read_to_string(Path::new(path))
            .await
            .map_err(|e| format!("Failed to read '{}': {}", path, e))?;

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();

        let start = args
            .get("start_line")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .saturating_sub(1) as usize;
        let end = args
            .get("end_line")
            .and_then(|v| v.as_u64())
            .map(|e| e as usize)
            .unwrap_or(total)
            .min(total);

        if start >= total || start >= end {
            return Err(format!(
                "Invalid line range {}..{} (file has {} lines)",
                start + 1,
                end,
                total
            ));
        }

        let excerpt = lines[start..end].join("\n");
        Ok(format!(
            "```\n{}:\n{}\n```\n(Lines {}-{} of {})",
            path,
            excerpt,
            start + 1,
            end,
            total
        ))
    }
}
