use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::provider::ModelOptions;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub channels: Vec<ChannelConfig>,
    pub providers: Vec<ProviderConfig>,
    pub agents: Vec<AgentConfig>,
}

impl Config {
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

    pub fn provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers
            .iter()
            .find(|provider| provider.name() == name)
    }

    pub fn agent(&self, name: &str) -> Option<&AgentConfig> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    pub fn model_options_for_agent(&self, agent_name: &str) -> Option<&ModelOptions> {
        let agent = self.agent(agent_name)?;
        self.provider(&agent.provider)?.model_options(&agent.model)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub name: String,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ChannelConfig {
    QQ {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ProviderConfig {
    OpenAI {
        name: String,
        api_key: String,
        models: Vec<ModelOptions>,
    },
    OpenAICodex {
        name: String,
        access_token: String,
        account_id: Option<String>,
        models: Vec<ModelOptions>,
        base_url: Option<String>,
    },
    DeepSeek {
        name: String,
        api_key: String,
        models: Vec<ModelOptions>,
        base_url: Option<String>,
    },
}

impl ProviderConfig {
    pub fn name(&self) -> &str {
        match self {
            ProviderConfig::OpenAI { name, .. }
            | ProviderConfig::OpenAICodex { name, .. }
            | ProviderConfig::DeepSeek { name, .. } => name,
        }
    }

    pub fn models(&self) -> &[ModelOptions] {
        match self {
            ProviderConfig::OpenAI { models, .. }
            | ProviderConfig::OpenAICodex { models, .. }
            | ProviderConfig::DeepSeek { models, .. } => models,
        }
    }

    pub fn default_model_options(&self) -> Option<&ModelOptions> {
        self.models().first()
    }

    pub fn model_options(&self, name: &str) -> Option<&ModelOptions> {
        self.models()
            .iter()
            .find(|model| model.model.as_deref() == Some(name))
    }
}

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
            "channels": [{ "type": "QQ" }],
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
                    "model": "deepseek-v4-pro"
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
                    "model": "missing-model"
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
