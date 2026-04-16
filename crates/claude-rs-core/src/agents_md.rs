use std::path::Path;
use tokio::fs;

const DEFAULT_LIMIT: usize = 32 * 1024; // 32 KiB

/// Load AGENTS.md and AGENTS.override.md files by walking from the current
/// directory up to the filesystem root.
///
/// Files are collected in order from root → current directory, with
/// `AGENTS.override.md` taking precedence over `AGENTS.md` at each level.
/// The total content is truncated to `limit` bytes.
pub async fn load_agents_md(cwd: &Path, limit: Option<usize>) -> anyhow::Result<String> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let mut paths = Vec::new();

    // Walk upward and collect directories.
    let mut current = Some(cwd);
    while let Some(dir) = current {
        paths.push(dir.to_path_buf());
        current = dir.parent();
    }

    // Reverse so we process root → cwd.
    paths.reverse();

    let mut contents = Vec::new();
    for dir in &paths {
        let override_path = dir.join("AGENTS.override.md");
        let normal_path = dir.join("AGENTS.md");

        if override_path.exists() {
            if let Ok(text) = fs::read_to_string(&override_path).await {
                contents.push((override_path, text));
            }
        } else if normal_path.exists() {
            if let Ok(text) = fs::read_to_string(&normal_path).await {
                contents.push((normal_path, text));
            }
        }
    }

    let mut combined = String::new();
    for (path, text) in contents {
        if !combined.is_empty() {
            combined.push_str("\n\n---\n\n");
        }
        combined.push_str(&format!("<!-- From: {} -->\n", path.display()));
        combined.push_str(&text);
        if combined.len() > limit {
            combined.truncate(limit);
            combined.push_str("\n\n[truncated]");
            break;
        }
    }

    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_agents_md_basic() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();

        let mut f1 = std::fs::File::create(root.join("AGENTS.md")).unwrap();
        writeln!(f1, "Root agents").unwrap();

        let mut f2 = std::fs::File::create(sub.join("AGENTS.md")).unwrap();
        writeln!(f2, "Sub agents").unwrap();

        let result = load_agents_md(&sub, None).await.unwrap();
        assert!(result.contains("Root agents"));
        assert!(result.contains("Sub agents"));
        assert!(result.contains("<!-- From:"));
    }
}
