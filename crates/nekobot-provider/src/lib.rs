//! Concrete provider implementations for the NekoBot framework.
//!
//! Provides [`DeepSeekProvider`] and [`OpenAiCodexProvider`], plus a
//! convenience function [`register_providers`] that registers both into a
//! [`ProviderRegistry`](nekobot_core::provider::ProviderRegistry).

use std::sync::Arc;

use nekobot_core::config::ProviderConfig;

pub mod deepseek;
pub mod openai_codex;
pub(crate) mod utils;

pub use deepseek::DeepSeekProvider;
pub use nekobot_core::provider::{
    ModelCapabilities, ModelOptions, Provider, ProviderError, ProviderEvent, ProviderRegistry,
    ProviderRequest,
};
pub use openai_codex::OpenAiCodexProvider;

/// Register the DeepSeek and OpenAI Codex provider factories into a registry.
///
/// Call this once at startup before [`NekoBot::run`](nekobot_core::NekoBot::run).
pub fn register_providers(
    registry: &mut nekobot_core::provider::ProviderRegistry,
) -> anyhow::Result<()> {
    registry.register("DeepSeek", |config| match config {
        ProviderConfig::DeepSeek {
            api_key,
            models,
            base_url,
            ..
        } => {
            let model = models
                .first()
                .and_then(|m| m.model.clone())
                .unwrap_or_default();
            let provider = DeepSeekProvider::from_config(
                api_key.clone(),
                model,
                base_url.as_deref().map(|s| s.to_owned()),
            )?;
            Ok(Arc::new(provider) as Arc<dyn Provider>)
        }
        _ => anyhow::bail!("expected DeepSeek provider config, got {}", config.name()),
    })?;

    registry.register("OpenAICodex", |config| match config {
        ProviderConfig::OpenAICodex {
            access_token,
            account_id,
            models,
            base_url,
            ..
        } => {
            let model = models
                .first()
                .and_then(|m| m.model.clone())
                .unwrap_or_default();
            let provider = OpenAiCodexProvider::from_config(
                access_token.clone(),
                account_id.clone(),
                model,
                base_url.as_deref().map(|s| s.to_owned()),
            )?;
            Ok(Arc::new(provider) as Arc<dyn Provider>)
        }
        _ => anyhow::bail!(
            "expected OpenAICodex provider config, got {}",
            config.name()
        ),
    })?;

    Ok(())
}
