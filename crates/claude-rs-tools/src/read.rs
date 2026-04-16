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
