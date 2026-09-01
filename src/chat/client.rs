use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::types::{ChatMessage, ChatRequest, ModelsResponse, StreamChunk};

/// Model ids that are not chat models (same filter Maple's client applies).
const NON_CHAT_MARKERS: &[&str] =
    &["whisper", "embed", "rerank", "transcription", "text-to-speech", "tts", "image-generation"];

#[derive(Clone)]
pub struct ChatClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl ChatClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self { http: reqwest::Client::new(), base_url, api_key }
    }

    /// Stream a chat completion, invoking `on_delta` per content token batch.
    /// Returns the finish_reason if the backend reported one.
    pub async fn stream_chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        cancel: CancellationToken,
        mut on_delta: impl FnMut(&str),
    ) -> Result<Option<String>> {
        let req = ChatRequest { model: model.to_string(), messages, stream: true };
        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await
            .context("sending chat completion request")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("chat completion failed ({status}): {}", body.chars().take(400).collect::<String>());
        }

        let mut finish_reason = None;
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Ok(Some("cancelled".into())),
                chunk = stream.next() => match chunk {
                    Some(c) => c.context("reading SSE stream")?,
                    None => break,
                },
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // SSE events are separated by a blank line; process complete lines only.
            while let Some(pos) = buf.find('\n') {
                let line: String = buf.drain(..=pos).collect();
                let line = line.trim();
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(finish_reason);
                }
                match serde_json::from_str::<StreamChunk>(data) {
                    Ok(parsed) => {
                        for choice in parsed.choices {
                            if let Some(text) = choice.delta.content.as_deref() {
                                if !text.is_empty() {
                                    on_delta(text);
                                }
                            }
                            if choice.finish_reason.is_some() {
                                finish_reason = choice.finish_reason;
                            }
                        }
                    }
                    Err(e) => tracing::warn!("unparseable SSE chunk ({e}): {data}"),
                }
            }
        }
        Ok(finish_reason)
    }

    /// List selectable chat models. maple-proxy 0.3.2 does not route
    /// /v1/models/catalog, so this filters /v1/models by id markers.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .http
            .get(format!("{}/v1/models", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("listing models")?
            .error_for_status()?;
        let models: ModelsResponse = resp.json().await?;
        let mut ids: Vec<String> = models
            .data
            .into_iter()
            .map(|m| m.id)
            .filter(|id| !NON_CHAT_MARKERS.iter().any(|m| id.contains(m)))
            .collect();
        // Hoist the default model to the front, like Maple's own picker.
        ids.sort_by_key(|id| id != crate::config::DEFAULT_MODEL);
        Ok(ids)
    }
}
