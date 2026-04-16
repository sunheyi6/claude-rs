use crate::Tool;
use async_trait::async_trait;
use serde_json::{Value, json};

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Find files matching a glob pattern. Respects .gitignore by default."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern, e.g. src/**/*.rs"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory (default: current directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, arguments: Value) -> anyhow::Result<String> {
        let pattern = arguments["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing pattern"))?;
        let base = arguments["path"].as_str().unwrap_or(".");

        let glob = globset::Glob::new(pattern)?.compile_matcher();
        let mut files = Vec::new();

        let walk = ignore::WalkBuilder::new(base)
            .standard_filters(true)
            .build();

        for entry in walk {
            let entry = entry?;
            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                let path = entry.path();
                let relative = path.strip_prefix(base).unwrap_or(path);
                if glob.is_match(relative) || glob.is_match(path) {
                    files.push(path.to_string_lossy().to_string());
                }
            }
        }

        files.sort();
        Ok(files.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        std::env::temp_dir().join(format!("claude_rs_glob_test_{name}_{stamp}"))
    }

    #[tokio::test]
    async fn test_glob_execute_normal_matches_expected_files() {
        let dir = temp_dir("normal");
        fs::create_dir_all(dir.join("src")).await.expect("create dir");
        fs::write(dir.join("src/a.rs"), "fn a() {}")
            .await
            .expect("write file");
        fs::write(dir.join("src/b.txt"), "x")
            .await
            .expect("write file");
        let tool = GlobTool;

        let output = tool
            .execute(json!({"pattern":"src/**/*.rs","path": dir.to_string_lossy()}))
            .await
            .expect("glob should succeed");

        assert!(output.contains("a.rs"));
        assert!(!output.contains("b.txt"));
        fs::remove_dir_all(dir).await.expect("cleanup");
    }

    #[tokio::test]
    async fn test_glob_execute_boundary_no_matches_returns_empty_string() {
        let dir = temp_dir("boundary");
        fs::create_dir_all(&dir).await.expect("create dir");
        let tool = GlobTool;

        let output = tool
            .execute(json!({"pattern":"**/*.md","path": dir.to_string_lossy()}))
            .await
            .expect("glob should succeed");

        assert_eq!(output, "");
        fs::remove_dir_all(dir).await.expect("cleanup");
    }

    #[tokio::test]
    async fn test_glob_execute_error_invalid_pattern() {
        let tool = GlobTool;
        let err = tool.execute(json!({"pattern":"["})).await;
        assert!(err.is_err());
    }
}
