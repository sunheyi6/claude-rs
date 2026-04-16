use crate::Tool;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::fs;

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Apply a precise text replacement in a file. old_text must match exactly."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "old_text": {
                    "type": "string",
                    "description": "Exact text to replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "Replacement text"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing path"))?;
        let old_text = arguments["old_text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing old_text"))?;
        let new_text = arguments["new_text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing new_text"))?;

        let content = fs::read_to_string(path).await?;

        if !content.contains(old_text) {
            anyhow::bail!("old_text not found in file");
        }

        let occurrences = content.matches(old_text).count();
        if occurrences > 1 {
            anyhow::bail!(
                "old_text appears {} times in the file; please provide more context",
                occurrences
            );
        }

        let new_content = content.replacen(old_text, new_text, 1);
        fs::write(path, new_content).await?;

        Ok(format!(
            "Successfully edited {} (replaced {} bytes with {} bytes)",
            path,
            old_text.len(),
            new_text.len()
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
        std::env::temp_dir().join(format!("claude_rs_edit_test_{name}_{stamp}.txt"))
    }

    #[tokio::test]
    async fn test_edit_execute_normal_single_replacement() {
        let path = temp_file("normal");
        fs::write(&path, "alpha beta gamma")
            .await
            .expect("write fixture");
        let tool = EditTool;

        let output = tool
            .execute(json!({
                "path": path.to_string_lossy(),
                "old_text": "beta",
                "new_text": "delta"
            }))
            .await
            .expect("edit should succeed");

        assert!(output.contains("Successfully edited"));
        let after = fs::read_to_string(&path).await.expect("read edited content");
        assert_eq!(after, "alpha delta gamma");
        fs::remove_file(path).await.expect("cleanup fixture");
    }

    #[tokio::test]
    async fn test_edit_execute_boundary_multiple_occurrences_rejected() {
        let path = temp_file("boundary");
        fs::write(&path, "same same").await.expect("write fixture");
        let tool = EditTool;

        let err = tool
            .execute(json!({
                "path": path.to_string_lossy(),
                "old_text": "same",
                "new_text": "x"
            }))
            .await;

        assert!(err.is_err());
        fs::remove_file(path).await.expect("cleanup fixture");
    }

    #[tokio::test]
    async fn test_edit_execute_error_old_text_missing() {
        let path = temp_file("error");
        fs::write(&path, "hello world").await.expect("write fixture");
        let tool = EditTool;

        let err = tool
            .execute(json!({
                "path": path.to_string_lossy(),
                "old_text": "missing",
                "new_text": "x"
            }))
            .await;

        assert!(err.is_err());
        fs::remove_file(path).await.expect("cleanup fixture");
    }
}
