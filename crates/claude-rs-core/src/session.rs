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
