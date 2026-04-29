//! Provider abstraction layer for LLM backends.
//!
//! Defines the [`Provider`] trait, request/response types, error types,
//! streaming events, and the [`ProviderRegistry`] factory pattern.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::mpsc::Sender;

use crate::agent::types::{ChatRequest, ChatResponse, Usage};
use crate::config::ProviderConfig;

/// A chat completion request combined with model options.
#[derive(Clone, Debug, Default)]
pub struct ProviderRequest {
    pub chat: ChatRequest,
    pub options: ModelOptions,
}

/// Per-request model configuration overrides.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelOptions {
    /// Model name override. Falls back to the provider's default model if `None`.
    pub model: Option<String>,
    pub capabilities: ModelCapabilities,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_output_tokens: Option<u32>,
    /// Provider-specific extra parameters forwarded as-is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Feature flags advertised by a model.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tools: bool,
    pub vision: bool,
    pub reasoning: bool,
}

/// Events emitted during a streaming completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    Started,
    ContentDelta(String),
    ReasoningDelta(String),
    Finished { usage: Option<Usage> },
}

/// Errors returned by provider implementations.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider authentication failed: {0}")]
    Authentication(String),

    #[error("provider rate limited: {message}")]
    RateLimited {
        retry_after: Option<Duration>,
        message: String,
    },

    #[error("invalid provider request: {0}")]
    InvalidRequest(String),

    #[error("unsupported provider feature: {0}")]
    UnsupportedFeature(String),

    #[error("provider request timed out: {0}")]
    Timeout(String),

    #[error("provider remote error: {0}")]
    Remote(String),

    #[error("provider unavailable: {0}")]
    Unavailable(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Trait for LLM provider backends.
///
/// Implementations wrap an HTTP client for a specific provider (DeepSeek,
/// OpenAI Codex, etc.) and translate between the framework's types and
/// the provider's wire protocol.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Return a unique identifier for this provider instance.
    fn id(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Execute a chat completion and return the full response.
    async fn complete(&self, request: ProviderRequest) -> Result<ChatResponse, ProviderError>;

    /// Execute a streaming chat completion.
    ///
    /// Events are sent through the `events` channel as they arrive. Returns
    /// `UnsupportedFeature` by default; implementors must override this.
    async fn stream(
        &self,
        _request: ProviderRequest,
        _events: Sender<ProviderEvent>,
    ) -> Result<ChatResponse, ProviderError> {
        Err(ProviderError::UnsupportedFeature("stream".to_owned()))
    }
}

/// Factory closure type for creating a provider from its config.
pub type ProviderCreateFn =
    Arc<dyn Fn(&ProviderConfig) -> anyhow::Result<Arc<dyn Provider>> + Send + Sync>;

/// Registry of named provider factories.
///
/// Maps provider type names (e.g. `"DeepSeek"`, `"OpenAICodex"`) to factory
/// closures. External crates (e.g. `nekobot-provider`) register factories;
/// the core uses them to instantiate providers from [`ProviderConfig`] entries.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    factories: HashMap<String, ProviderCreateFn>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider factory under the given name.
    ///
    /// The name should match the `type` field in [`ProviderConfig`] JSON
    /// (e.g. `"DeepSeek"`, `"OpenAICodex"`).
    pub fn register<F>(&mut self, name: impl Into<String>, create: F) -> anyhow::Result<()>
    where
        F: Fn(&ProviderConfig) -> anyhow::Result<Arc<dyn Provider>> + Send + Sync + 'static,
    {
        let name = name.into();
        if name.trim().is_empty() {
            anyhow::bail!("provider factory name cannot be empty");
        }

        if self.factories.contains_key(&name) {
            anyhow::bail!("duplicate provider factory: {name}");
        }

        self.factories.insert(name, Arc::new(create));
        Ok(())
    }

    /// Create a provider from its config, looking up the factory by name.
    ///
    /// Returns `Ok(None)` if no factory is registered for the config's type.
    pub fn create(&self, config: &ProviderConfig) -> anyhow::Result<Option<Arc<dyn Provider>>> {
        let Some(factory) = self.factories.get(config.name()) else {
            return Ok(None);
        };

        factory(config)
            .with_context(|| format!("failed to create provider {}", config.name()))
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_capabilities_default_declares_no_capabilities() {
        let capabilities = ModelCapabilities::default();

        assert!(!capabilities.streaming);
        assert!(!capabilities.tools);
        assert!(!capabilities.vision);
        assert!(!capabilities.reasoning);
    }

    struct TestProvider;

    #[async_trait::async_trait]
    impl Provider for TestProvider {
        async fn complete(&self, _request: ProviderRequest) -> Result<ChatResponse, ProviderError> {
            Ok(ChatResponse {
                content: String::new(),
                reasoning_content: None,
                images: Vec::new(),
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn default_stream_returns_unsupported_feature() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let result = TestProvider
            .stream(ProviderRequest::default(), sender)
            .await;

        assert!(matches!(
            result,
            Err(ProviderError::UnsupportedFeature(feature)) if feature == "stream"
        ));
    }
}
