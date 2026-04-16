use claude_rs_llm::ToolCall;
use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl PermissionMode {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "read-only" | "readonly" | "ro" => Some(Self::ReadOnly),
            "workspace-write" | "workspace" | "ww" => Some(Self::WorkspaceWrite),
            "danger-full-access" | "danger" | "full" => Some(Self::DangerFullAccess),
            _ => None,
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "read-only"),
            Self::WorkspaceWrite => write!(f, "workspace-write"),
            Self::DangerFullAccess => write!(f, "danger-full-access"),
        }
    }
}

pub fn enforce_tool_permission(
    mode: PermissionMode,
    workspace_root: &Path,
    call: &ToolCall,
) -> anyhow::Result<()> {
    match mode {
        PermissionMode::DangerFullAccess => Ok(()),
        PermissionMode::ReadOnly => enforce_read_only(workspace_root, call),
        PermissionMode::WorkspaceWrite => enforce_workspace_write(workspace_root, call),
    }
}

fn enforce_read_only(workspace_root: &Path, call: &ToolCall) -> anyhow::Result<()> {
    match call.name.as_str() {
        "read" | "grep" | "glob" => {
            if let Some(path) = call.arguments.get("path").and_then(|v| v.as_str()) {
                ensure_in_workspace(workspace_root, path)?;
            }
            Ok(())
        }
        "todo_write" => Ok(()),
        "bash" | "write" | "edit" | "task" => anyhow::bail!(
            "Permission denied in read-only mode: tool `{}` is blocked",
            call.name
        ),
        _ => anyhow::bail!("Permission denied: unknown tool `{}`", call.name),
    }
}

fn enforce_workspace_write(workspace_root: &Path, call: &ToolCall) -> anyhow::Result<()> {
    match call.name.as_str() {
        "read" | "grep" | "glob" | "write" | "edit" => {
            if let Some(path) = call.arguments.get("path").and_then(|v| v.as_str()) {
                ensure_in_workspace(workspace_root, path)?;
            }
            Ok(())
        }
        "todo_write" => Ok(()),
        "bash" | "task" => anyhow::bail!(
            "Permission denied in workspace-write mode: tool `{}` is blocked",
            call.name
        ),
        _ => anyhow::bail!("Permission denied: unknown tool `{}`", call.name),
    }
}

fn ensure_in_workspace(workspace_root: &Path, requested_path: &str) -> anyhow::Result<()> {
    let root = normalize_path(workspace_root, ".");
    let candidate = normalize_path(workspace_root, requested_path);
    if candidate.starts_with(&root) {
        Ok(())
    } else {
        anyhow::bail!(
            "Permission denied: path `{}` is outside workspace `{}`",
            candidate.display(),
            root.display()
        )
    }
}

fn normalize_path(base: &Path, path: &str) -> PathBuf {
    let raw = Path::new(path);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    };

    let mut normalized = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(comp),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "t1".to_string(),
            name: name.to_string(),
            arguments,
        }
    }

    #[test]
    fn readonly_blocks_write_tools() {
        let root = Path::new("C:/repo");
        let result = enforce_tool_permission(
            PermissionMode::ReadOnly,
            root,
            &call("write", json!({"path":"src/main.rs","content":"x"})),
        );
        assert!(result.is_err());
    }

    #[test]
    fn workspace_write_allows_write_within_root() {
        let root = Path::new("C:/repo");
        let result = enforce_tool_permission(
            PermissionMode::WorkspaceWrite,
            root,
            &call("write", json!({"path":"src/main.rs","content":"x"})),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn workspace_write_blocks_path_escape() {
        let root = Path::new("C:/repo");
        let result = enforce_tool_permission(
            PermissionMode::WorkspaceWrite,
            root,
            &call("read", json!({"path":"../secret.txt"})),
        );
        assert!(result.is_err());
    }

    #[test]
    fn parse_modes() {
        assert_eq!(
            PermissionMode::parse("read-only"),
            Some(PermissionMode::ReadOnly)
        );
        assert_eq!(
            PermissionMode::parse("workspace"),
            Some(PermissionMode::WorkspaceWrite)
        );
        assert_eq!(
            PermissionMode::parse("danger"),
            Some(PermissionMode::DangerFullAccess)
        );
    }
}
