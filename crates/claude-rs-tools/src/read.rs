use crate::Tool;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::fs;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Supports optional offset and limit."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start from (1-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing path"))?;
        let content = fs::read_to_string(path).await?;

        let offset = arguments["offset"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
        let limit = arguments["limit"].as_u64().unwrap_or(2000) as usize;

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let end = (offset + limit).min(total);

        if offset >= total {
            return Ok(format!(
                "File has {} lines. Offset {} is out of range.",
                total,
                offset + 1
            ));
        }

        let selected = &lines[offset..end];
        let mut result = String::new();
        for (i, line) in selected.iter().enumerate() {
            result.push_str(&format!("{:4} | {}\n", offset + i + 1, line));
        }

        if end < total {
            result.push_str(&format!("\n... {} more lines ...", total - end));
        }

        Ok(result.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::fs;

    fn temp_file(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        std::env::temp_dir().join(format!("claude_rs_read_test_{name}_{stamp}.txt"))
    }

    #[tokio::test]
    async fn test_read_execute_normal_reads_selected_lines() {
        let path = temp_file("normal");
        fs::write(&path, "a\nb\nc\nd\n").await.expect("write fixture");
        let tool = ReadTool;

        let output = tool
            .execute(json!({"path": path.to_string_lossy(), "offset": 2, "limit": 2}))
            .await
            .expect("read should succeed");

        assert!(output.contains("2 | b"));
        assert!(output.contains("3 | c"));
        assert!(!output.contains("1 | a"));
        fs::remove_file(path).await.expect("cleanup fixture");
    }

    #[tokio::test]
    async fn test_read_execute_boundary_offset_out_of_range() {
        let path = temp_file("boundary");
        fs::write(&path, "only\n").await.expect("write fixture");
        let tool = ReadTool;

        let output = tool
            .execute(json!({"path": path.to_string_lossy(), "offset": 99}))
            .await
            .expect("read should return message");

        assert!(output.contains("out of range"));
        fs::remove_file(path).await.expect("cleanup fixture");
    }

    #[tokio::test]
    async fn test_read_execute_error_missing_path() {
        let tool = ReadTool;
        let err = tool.execute(json!({})).await;
        assert!(err.is_err());
    }
}
