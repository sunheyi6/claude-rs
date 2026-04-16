use crate::Tool;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::fs;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write content to a file. Creates parent directories if needed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing path"))?;
        let content = arguments["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing content"))?;

        if let Some(parent) = std::path::Path::new(path).parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(path, content).await?;
        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            path
        ))
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
        std::env::temp_dir()
            .join(format!("claude_rs_write_test_{name}_{stamp}"))
            .join("nested.txt")
    }

    #[tokio::test]
    async fn test_write_execute_normal_creates_parent_and_writes() {
        let path = temp_file("normal");
        let tool = WriteTool;
        let content = "hello world";

        let output = tool
            .execute(json!({"path": path.to_string_lossy(), "content": content}))
            .await
            .expect("write should succeed");

        assert!(output.contains("Successfully wrote"));
        let saved = fs::read_to_string(&path).await.expect("should read saved file");
        assert_eq!(saved, content);
        if let Some(parent) = path.parent() {
            fs::remove_dir_all(parent).await.expect("cleanup fixture");
        }
    }

    #[tokio::test]
    async fn test_write_execute_boundary_empty_content() {
        let path = temp_file("boundary");
        let tool = WriteTool;

        let output = tool
            .execute(json!({"path": path.to_string_lossy(), "content": ""}))
            .await
            .expect("empty content is still valid");

        assert!(output.contains("0 bytes"));
        let saved = fs::read_to_string(&path).await.expect("should read saved file");
        assert_eq!(saved, "");
        if let Some(parent) = path.parent() {
            fs::remove_dir_all(parent).await.expect("cleanup fixture");
        }
    }

    #[tokio::test]
    async fn test_write_execute_error_missing_content() {
        let tool = WriteTool;
        let err = tool.execute(json!({"path": "abc.txt"})).await;
        assert!(err.is_err());
    }
}
