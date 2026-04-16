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
