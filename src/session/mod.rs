use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::chat::types::ChatMessage;

/// Chat sessions persist locally (private by design — nothing leaves the
/// machine unencrypted). Agent sessions are persisted by goose itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub model: String,
    pub updated_at: String,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
}

fn sessions_dir() -> PathBuf {
    crate::config::data_dir().join("sessions")
}

fn path_for(id: &str) -> PathBuf {
    sessions_dir().join(format!("{id}.json"))
}

fn now() -> String {
    // Seconds precision is plenty for a picker.
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", d.as_secs())
}

pub fn save(id: &str, model: &str, messages: &[ChatMessage]) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let title = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.chars().take(60).collect::<String>())
        .unwrap_or_else(|| "untitled".into());
    let session = Session {
        id: id.to_string(),
        title,
        model: model.to_string(),
        updated_at: now(),
        messages: messages.to_vec(),
    };
    let dir = sessions_dir();
    fs::create_dir_all(&dir)?;
    let path = path_for(id);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&session)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load(id: &str) -> Result<Session> {
    let raw = fs::read_to_string(path_for(id))
        .with_context(|| format!("reading session {id}"))?;
    Ok(serde_json::from_str(&raw)?)
}

/// Newest first, by file mtime.
pub fn list() -> Vec<SessionMeta> {
    let Ok(entries) = fs::read_dir(sessions_dir()) else { return Vec::new() };
    let mut sessions: Vec<(std::time::SystemTime, SessionMeta)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            let raw = fs::read_to_string(e.path()).ok()?;
            let s: Session = serde_json::from_str(&raw).ok()?;
            Some((mtime, SessionMeta { id: s.id, title: s.title }))
        })
        .collect();
    sessions.sort_by(|a, b| b.0.cmp(&a.0));
    sessions.into_iter().map(|(_, m)| m).collect()
}
