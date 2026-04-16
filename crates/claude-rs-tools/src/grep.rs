use crate::Tool;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search file contents using ripgrep. Returns matching lines with file paths and line numbers."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: current directory)"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines to show around each match"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let pattern = arguments["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing pattern"))?;
        let path = arguments["path"].as_str().unwrap_or(".");
        let case_insensitive = arguments["case_insensitive"].as_bool().unwrap_or(false);
        let context_lines = arguments["context_lines"].as_u64().unwrap_or(0) as usize;
        let max_results = arguments["max_results"].as_u64().unwrap_or(250) as usize;

        // Use rg if available, otherwise fall back to PowerShell/findstr on Windows or grep on Unix
        let rg_available = Command::new("rg")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        let output = if rg_available {
            let mut cmd = Command::new("rg");
            cmd.arg("--line-number")
                .arg("--with-filename")
                .arg(pattern)
                .arg(path);
            if case_insensitive {
                cmd.arg("--ignore-case");
            }
            if context_lines > 0 {
                cmd.arg(format!("--context={}", context_lines));
            }
            cmd.arg("--max-count").arg(format!("{}", max_results));
            cmd.output().await?
        } else {
            // Fallback for Windows without ripgrep
            #[cfg(target_os = "windows")]
            {
                let mut cmd = Command::new("findstr");
                cmd.arg("/N").arg(pattern).arg(path);
                if case_insensitive {
                    cmd.arg("/I");
                }
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
                cmd.output().await?
            }
            #[cfg(not(target_os = "windows"))]
            {
                let mut cmd = Command::new("grep");
                cmd.arg("-rn").arg(pattern).arg(path);
                if case_insensitive {
                    cmd.arg("-i");
                }
                cmd.output().await?
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() && stdout.is_empty() {
            if stderr.is_empty() {
                return Ok("No matches found.".to_string());
            }
            anyhow::bail!("grep failed: {}", stderr);
        }

        let lines: Vec<&str> = stdout.lines().collect();
        let mut result = lines[..lines.len().min(max_results)].join("\n");
        if lines.len() > max_results {
            result.push_str(&format!(
                "\n... {} more matches ...",
                lines.len() - max_results
            ));
        }

        Ok(result)
    }
}
