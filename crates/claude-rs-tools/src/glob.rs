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
