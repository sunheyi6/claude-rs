use claude_rs_llm::Message;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<Message>,
}

pub fn session_dir() -> anyhow::Result<PathBuf> {
    let base = dirs::data_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow::anyhow!("could not find home directory"))?;
    Ok(base.join(".claude-rs").join("sessions"))
}

pub async fn ensure_session_dir() -> anyhow::Result<PathBuf> {
    let dir = session_dir()?;
    fs::create_dir_all(&dir).await?;
    Ok(dir)
}

pub async fn save_session(session: &Session) -> anyhow::Result<()> {
    let dir = ensure_session_dir().await?;
    let path = dir.join(format!("{}.json", session.id));
    let json = serde_json::to_string_pretty(session)?;
    fs::write(path, json).await?;
    Ok(())
}

pub async fn list_sessions() -> anyhow::Result<Vec<Session>> {
    let dir = session_dir()?;
    let mut sessions = Vec::new();

    let mut entries = fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(text) = fs::read_to_string(&path).await {
                if let Ok(session) = serde_json::from_str::<Session>(&text) {
                    sessions.push(session);
                }
            }
        }
    }

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
}

pub async fn load_session(id: &str) -> anyhow::Result<Session> {
    let dir = session_dir()?;
    let path = dir.join(format!("{}.json", id));
    let text = fs::read_to_string(&path).await?;
    let session = serde_json::from_str(&text)?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(id: &str, updated_at: &str) -> Session {
        Session {
            id: id.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: updated_at.to_string(),
            messages: vec![Message::user("hello")],
        }
    }

    #[tokio::test]
    async fn test_save_and_load_session_normal_roundtrip() {
        let session = make_session("unit-session-normal", "2026-01-01T00:00:00Z");
        save_session(&session).await.expect("save should succeed");

        let loaded = load_session(&session.id).await.expect("load should succeed");
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.updated_at, session.updated_at);

        let dir = session_dir().expect("session dir");
        let _ = fs::remove_file(dir.join(format!("{}.json", session.id))).await;
    }

    #[tokio::test]
    async fn test_list_sessions_boundary_sorted_descending() {
        let s1 = make_session("unit-session-sort-a", "2026-01-01T00:00:00Z");
        let s2 = make_session("unit-session-sort-b", "2026-01-02T00:00:00Z");
        save_session(&s1).await.expect("save should succeed");
        save_session(&s2).await.expect("save should succeed");

        let sessions = list_sessions().await.expect("list should succeed");
        let a = sessions
            .iter()
            .position(|s| s.id == "unit-session-sort-a")
            .expect("a should be present");
        let b = sessions
            .iter()
            .position(|s| s.id == "unit-session-sort-b")
            .expect("b should be present");
        assert!(b < a);

        let dir = session_dir().expect("session dir");
        let _ = fs::remove_file(dir.join("unit-session-sort-a.json")).await;
        let _ = fs::remove_file(dir.join("unit-session-sort-b.json")).await;
    }

    #[tokio::test]
    async fn test_load_session_error_missing_id() {
        let result = load_session("unit-session-not-found").await;
        assert!(result.is_err());
    }
}
