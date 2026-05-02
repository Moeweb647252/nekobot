//! Embedding API client — calls OpenAI-compatible `/v1/embeddings` endpoints.

use std::sync::Arc;

use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Clone)]
pub struct EmbeddingClient {
    http: Arc<Client>,
    api_url: String,
    api_key: String,
    model: String,
}

impl EmbeddingClient {
    pub fn new(api_url: String, api_key: String, model: String) -> Self {
        Self {
            http: Arc::new(Client::new()),
            api_url,
            api_key,
            model,
        }
    }

    /// Generate an embedding vector for the given text.
    pub async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let resp = self
            .http
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "model": self.model,
                "input": text,
            }))
            .send()
            .await
            .context("embedding request failed")?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .context("failed to read embedding response")?;

        if !status.is_success() {
            anyhow::bail!("embedding API returned {status}: {body}");
        }

        let result: EmbeddingResponse =
            serde_json::from_str(&body).context("failed to parse embedding response")?;

        result
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| anyhow::anyhow!("empty embedding response"))
    }
}

use anyhow::Context;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_embedding_response() {
        let json = r#"{"data":[{"embedding":[0.1, -0.2, 0.3]}]}"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data[0].embedding, vec![0.1, -0.2, 0.3]);
    }
}
