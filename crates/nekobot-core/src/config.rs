//! Configuration types for the nekobot application, including providers,
//! agents, channels, middlewares, and serializable validation logic.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::provider::ModelOptions;

/// Top-level application configuration listing all channels, providers, and agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub channels: Vec<ChannelConfig>,
    pub providers: Vec<ProviderConfig>,
    pub agents: Vec<AgentConfig>,
    /// Optional SHA256 hex hash of the global login password.
    /// When set, C2C users must `/login <password>` before accessing any agent.
    #[serde(default)]
    pub password_hash: Option<String>,
    /// Path to the libSQL database file. Defaults to `"nekobot.db"`.
    #[serde(default = "default_database_path")]
    pub database_path: String,
}

fn default_database_path() -> String {
    "nekobot.db".to_owned()
}

impl Config {
    /// Validates the entire configuration, checking for duplicate/empty names,
    /// missing models, unknown provider references, and invalid middlewares.
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        let mut provider_names = HashSet::new();

        for provider in &self.providers {
            let provider_name = provider.name();
            if provider_name.trim().is_empty() {
                return Err(ConfigValidationError::EmptyProviderName);
            }

            if !provider_names.insert(provider_name.to_owned()) {
                return Err(ConfigValidationError::DuplicateProviderName(
                    provider_name.to_owned(),
                ));
            }

            if provider.models().is_empty() {
                return Err(ConfigValidationError::EmptyProviderModels {
                    provider: provider_name.to_owned(),
                });
            }

            for model in provider.models() {
                let model_name = model.model.as_deref().unwrap_or_default();
                if model_name.trim().is_empty() {
                    return Err(ConfigValidationError::EmptyProviderModel {
                        provider: provider_name.to_owned(),
                    });
                }
            }
        }

        let mut agent_names = HashSet::new();

        for agent in &self.agents {
            if agent.name.trim().is_empty() {
                return Err(ConfigValidationError::EmptyAgentName);
            }

            if !agent_names.insert(agent.name.clone()) {
                return Err(ConfigValidationError::DuplicateAgentName(
                    agent.name.clone(),
                ));
            }

            if agent.provider.trim().is_empty() {
                return Err(ConfigValidationError::EmptyAgentProvider {
                    agent: agent.name.clone(),
                });
            }

            if agent.model.trim().is_empty() {
                return Err(ConfigValidationError::EmptyAgentModel {
                    agent: agent.name.clone(),
                });
            }

            for middleware in &agent.middlewares {
                middleware.validate(&agent.name)?;
            }

            let provider = self.provider(&agent.provider).ok_or_else(|| {
                ConfigValidationError::UnknownAgentProvider {
                    agent: agent.name.clone(),
                    provider: agent.provider.clone(),
                }
            })?;

            if provider.model_options(&agent.model).is_none() {
                return Err(ConfigValidationError::UnknownAgentModel {
                    agent: agent.name.clone(),
                    provider: agent.provider.clone(),
                    model: agent.model.clone(),
                });
            }
        }

        Ok(())
    }

    /// Looks up a provider by its configured name.
    pub fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers
            .iter()
            .find(|provider| provider.name() == name)
    }

    /// Looks up an agent by its configured name.
    pub fn agent(&self, name: &str) -> Option<&AgentConfig> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    /// Resolves the full model options for a named agent by first finding the
    /// agent config and then looking up its model within the referenced provider.
    pub fn model_options_for_agent(&self, agent_name: &str) -> Option<&ModelOptions> {
        let agent = self.agent(agent_name)?;
        self.provider(&agent.provider)?.model_options(&agent.model)
    }
}

/// Configuration for a single agent, referencing a provider, model, and
/// an ordered list of middlewares.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub middlewares: Vec<MiddlewareConfig>,
}

/// Configuration for a single middleware, identified by name with additional
/// properties flattened from the serialized form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MiddlewareConfig {
    pub name: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl MiddlewareConfig {
    fn validate(&self, agent: &str) -> Result<(), ConfigValidationError> {
        if self.name.trim().is_empty() {
            return Err(ConfigValidationError::EmptyMiddlewareName {
                agent: agent.to_owned(),
            });
        }

        Ok(())
    }
}

/// Available chat channel integrations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ChannelConfig {
    /// QQ Bot channel using the official QQ Bot API.
    QQ {
        app_id: String,
        client_secret: String,
    },
}

impl ChannelConfig {
    /// Returns the channel type tag as it appears in the JSON `"type"` field.
    pub fn name(&self) -> &str {
        match self {
            ChannelConfig::QQ { .. } => "QQ",
        }
    }
}

/// Supported LLM provider configurations, each with a name, credentials, and
/// a list of available models.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ProviderConfig {
    /// Standard OpenAI API provider (chat completions).
    OpenAI {
        name: String,
        api_key: String,
        models: Vec<ModelOptions>,
    },
    /// OpenAI Codex provider using access-token-based authentication with an
    /// optional account ID and custom base URL.
    OpenAICodex {
        name: String,
        access_token: String,
        account_id: Option<String>,
        models: Vec<ModelOptions>,
        base_url: Option<String>,
    },
    /// DeepSeek API provider with an optional custom base URL.
    DeepSeek {
        name: String,
        api_key: String,
        models: Vec<ModelOptions>,
        base_url: Option<String>,
    },
}

impl ProviderConfig {
    /// Returns the provider type tag as it appears in the YAML `type` field
    /// (e.g. `"DeepSeek"`, `"OpenAICodex"`). Used for registry look-up.
    pub fn type_name(&self) -> &str {
        match self {
            ProviderConfig::OpenAI { .. } => "OpenAI",
            ProviderConfig::OpenAICodex { .. } => "OpenAICodex",
            ProviderConfig::DeepSeek { .. } => "DeepSeek",
        }
    }

    /// Returns the user-defined provider name used in agent config `provider` references.
    pub fn name(&self) -> &str {
        match self {
            ProviderConfig::OpenAI { name, .. }
            | ProviderConfig::OpenAICodex { name, .. }
            | ProviderConfig::DeepSeek { name, .. } => name,
        }
    }

    /// Returns the list of models configured for this provider.
    pub fn models(&self) -> &[ModelOptions] {
        match self {
            ProviderConfig::OpenAI { models, .. }
            | ProviderConfig::OpenAICodex { models, .. }
            | ProviderConfig::DeepSeek { models, .. } => models,
        }
    }

    /// Returns the first model in the list as the default.
    pub fn default_model_options(&self) -> Option<&ModelOptions> {
        self.models().first()
    }

    /// Looks up a specific model by name within this provider.
    pub fn model_options(&self, name: &str) -> Option<&ModelOptions> {
        self.models()
            .iter()
            .find(|model| model.model.as_deref() == Some(name))
    }
}

/// Errors that can occur during configuration validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("provider name cannot be empty")]
    EmptyProviderName,

    #[error("duplicate provider name: {0}")]
    DuplicateProviderName(String),

    #[error("provider {provider} must include at least one model")]
    EmptyProviderModels { provider: String },

    #[error("provider {provider} has a model with an empty name")]
    EmptyProviderModel { provider: String },

    #[error("agent name cannot be empty")]
    EmptyAgentName,

    #[error("duplicate agent name: {0}")]
    DuplicateAgentName(String),

    #[error("agent {agent} must reference a provider")]
    EmptyAgentProvider { agent: String },

    #[error("agent {agent} must reference a model")]
    EmptyAgentModel { agent: String },

    #[error("agent {agent} has a middleware with an empty name")]
    EmptyMiddlewareName { agent: String },

    #[error("agent {agent} references unknown provider {provider}")]
    UnknownAgentProvider { agent: String, provider: String },

    #[error("agent {agent} references unknown model {model} for provider {provider}")]
    UnknownAgentModel {
        agent: String,
        provider: String,
        model: String,
    },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn openai_provider_config_deserializes_with_models() {
        let config: ProviderConfig = serde_json::from_value(json!({
            "type": "OpenAI",
            "name": "openai",
            "api_key": "sk-test",
            "models": [
                {
                    "model": "gpt-5.4",
                    "capabilities": {
                        "streaming": true,
                        "tools": true
                    }
                }
            ],
        }))
        .unwrap();

        match config {
            ProviderConfig::OpenAI {
                name,
                api_key,
                models,
            } => {
                assert_eq!(name, "openai");
                assert_eq!(api_key, "sk-test");
                assert_eq!(models[0].model.as_deref(), Some("gpt-5.4"));
                assert!(models[0].capabilities.streaming);
                assert!(models[0].capabilities.tools);
            }
            ProviderConfig::OpenAICodex { .. } | ProviderConfig::DeepSeek { .. } => {
                panic!("expected OpenAI config")
            }
        }
    }

    #[test]
    fn openai_codex_provider_config_deserializes_with_models() {
        let config: ProviderConfig = serde_json::from_value(json!({
            "type": "OpenAICodex",
            "name": "codex",
            "access_token": "token",
            "account_id": "acct",
            "models": [
                {
                    "model": "gpt-5.2-codex",
                    "temperature": 0.3
                }
            ],
            "base_url": "https://example.test/backend-api/codex",
        }))
        .unwrap();

        match config {
            ProviderConfig::OpenAICodex {
                name,
                access_token,
                account_id,
                models,
                base_url,
            } => {
                assert_eq!(name, "codex");
                assert_eq!(access_token, "token");
                assert_eq!(account_id.as_deref(), Some("acct"));
                assert_eq!(models[0].model.as_deref(), Some("gpt-5.2-codex"));
                assert_eq!(models[0].temperature, Some(0.3));
                assert_eq!(
                    base_url.as_deref(),
                    Some("https://example.test/backend-api/codex")
                );
            }
            ProviderConfig::OpenAI { .. } | ProviderConfig::DeepSeek { .. } => {
                panic!("expected OpenAICodex config")
            }
        }
    }

    #[test]
    fn provider_config_requires_models() {
        let result = serde_json::from_value::<ProviderConfig>(json!({
            "type": "OpenAICodex",
            "name": "codex",
            "access_token": "token",
        }));

        assert!(result.is_err());
    }

    #[test]
    fn provider_config_rejects_old_model_field() {
        let result = serde_json::from_value::<ProviderConfig>(json!({
            "type": "DeepSeek",
            "name": "deepseek",
            "api_key": "sk-test",
            "model": "deepseek-v4-pro",
            "models": [{ "model": "deepseek-v4-pro" }],
        }));

        assert!(result.is_err());
    }

    #[test]
    fn deepseek_provider_config_deserializes_with_models() {
        let config: ProviderConfig = serde_json::from_value(json!({
            "type": "DeepSeek",
            "name": "deepseek",
            "api_key": "sk-test",
            "models": [
                {
                    "model": "deepseek-v4-pro",
                    "max_output_tokens": 1024,
                    "thinking": { "type": "enabled" }
                }
            ],
            "base_url": "https://api.deepseek.com",
        }))
        .unwrap();

        match config {
            ProviderConfig::DeepSeek {
                name,
                api_key,
                models,
                base_url,
            } => {
                assert_eq!(name, "deepseek");
                assert_eq!(api_key, "sk-test");
                assert_eq!(models[0].model.as_deref(), Some("deepseek-v4-pro"));
                assert_eq!(models[0].max_output_tokens, Some(1024));
                assert_eq!(models[0].extra["thinking"], json!({ "type": "enabled" }));
                assert_eq!(base_url.as_deref(), Some("https://api.deepseek.com"));
            }
            ProviderConfig::OpenAI { .. } | ProviderConfig::OpenAICodex { .. } => {
                panic!("expected DeepSeek config")
            }
        }
    }

    #[test]
    fn config_validates_agent_provider_and_model_references() {
        let config: Config = serde_json::from_value(json!({
            "channels": [{ "type": "QQ", "app_id": "test-app-id", "client_secret": "test-secret" }],
            "providers": [
                {
                    "type": "DeepSeek",
                    "name": "deepseek",
                    "api_key": "sk-test",
                    "models": [{ "model": "deepseek-v4-pro" }]
                }
            ],
            "agents": [
                {
                    "name": "Neko",
                    "provider": "deepseek",
                    "model": "deepseek-v4-pro",
                    "middlewares": []
                }
            ]
        }))
        .unwrap();

        config.validate().unwrap();
        let model_options = config.model_options_for_agent("Neko").unwrap();
        assert_eq!(model_options.model.as_deref(), Some("deepseek-v4-pro"));
    }

    #[test]
    fn config_rejects_unknown_agent_model() {
        let config: Config = serde_json::from_value(json!({
            "channels": [],
            "providers": [
                {
                    "type": "DeepSeek",
                    "name": "deepseek",
                    "api_key": "sk-test",
                    "models": [{ "model": "deepseek-v4-pro" }]
                }
            ],
            "agents": [
                {
                    "name": "Neko",
                    "provider": "deepseek",
                    "model": "missing-model",
                    "middlewares": []
                }
            ]
        }))
        .unwrap();

        assert!(matches!(
            config.validate(),
            Err(ConfigValidationError::UnknownAgentModel { .. })
        ));
    }

    #[test]
    fn agent_config_requires_middlewares() {
        let result = serde_json::from_value::<AgentConfig>(json!({
            "name": "Neko",
            "provider": "deepseek",
            "model": "deepseek-v4-pro"
        }));

        assert!(result.is_err());
    }

    #[test]
    fn middleware_config_deserializes_name_and_flattened_data() {
        let config: MiddlewareConfig = serde_json::from_value(json!({
            "name": "memory",
            "path": "./memory.db",
            "limit": 8
        }))
        .unwrap();

        assert_eq!(config.name, "memory");
        assert_eq!(config.data["path"], json!("./memory.db"));
        assert_eq!(config.data["limit"], json!(8));
    }

    #[test]
    fn middleware_config_requires_name() {
        let result = serde_json::from_value::<MiddlewareConfig>(json!({
            "path": "./memory.db"
        }));

        assert!(result.is_err());
    }

    #[test]
    fn config_rejects_empty_middleware_name() {
        let config: Config = serde_json::from_value(json!({
            "channels": [],
            "providers": [
                {
                    "type": "DeepSeek",
                    "name": "deepseek",
                    "api_key": "sk-test",
                    "models": [{ "model": "deepseek-v4-pro" }]
                }
            ],
            "agents": [
                {
                    "name": "Neko",
                    "provider": "deepseek",
                    "model": "deepseek-v4-pro",
                    "middlewares": [{ "name": "" }]
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            config.validate(),
            Err(ConfigValidationError::EmptyMiddlewareName {
                agent: "Neko".to_owned(),
            })
        );
    }

    #[test]
    fn config_rejects_empty_provider_model_name() {
        let config: Config = serde_json::from_value(json!({
            "channels": [],
            "providers": [
                {
                    "type": "DeepSeek",
                    "name": "deepseek",
                    "api_key": "sk-test",
                    "models": [{ "temperature": 0.2 }]
                }
            ],
            "agents": []
        }))
        .unwrap();

        assert_eq!(
            config.validate(),
            Err(ConfigValidationError::EmptyProviderModel {
                provider: "deepseek".to_owned(),
            })
        );
    }
}
