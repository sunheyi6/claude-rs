use crate::Tool;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::process::Command;

pub struct BashTool {
    pub timeout: Duration,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command. Returns stdout and stderr."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let cmd_str = arguments["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing command"))?;
        let timeout = arguments["timeout_ms"]
            .as_u64()
            .map(Duration::from_millis)
            .unwrap_or(self.timeout);

        let shell = if cfg!(target_os = "windows") {
            "powershell.exe"
        } else {
            "bash"
        };
        let flag = if cfg!(target_os = "windows") {
            "-Command"
        } else {
            "-c"
        };

        let output =
            tokio::time::timeout(timeout, Command::new(shell).arg(flag).arg(cmd_str).output())
                .await
                .map_err(|_| anyhow::anyhow!("command timed out after {:?}", timeout))?
                .map_err(|e| anyhow::anyhow!("failed to spawn command: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output.status.code().unwrap_or(-1);

        let mut result = format!("exit code: {}\n", code);
        if !stdout.is_empty() {
            result.push_str(&format!("stdout:\n{}\n", stdout));
        }
        if !stderr.is_empty() {
            result.push_str(&format!("stderr:\n{}\n", stderr));
        }
        Ok(result.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    async fn test_bash_execute_normal_returns_stdout() {
        let tool = BashTool::default();
        let output = tool
            .execute(json!({"command":"Write-Output hello"}))
            .await
            .expect("command should run");
        assert!(output.contains("exit code: 0"));
        assert!(output.contains("hello"));
    }

    #[tokio::test]
    async fn test_bash_execute_boundary_empty_stdout() {
        let tool = BashTool::default();
        let output = tool
            .execute(json!({"command":"$null"}))
            .await
            .expect("command should run");
        assert!(output.contains("exit code: 0"));
    }

    #[tokio::test]
    async fn test_bash_execute_error_timeout() {
        let tool = BashTool {
            timeout: Duration::from_millis(10),
        };
        let result = tool
            .execute(json!({"command":"Start-Sleep -Milliseconds 200","timeout_ms":10}))
            .await;
        assert!(result.is_err());
    }
}
