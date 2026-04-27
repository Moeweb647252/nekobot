use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub channels: Vec<ChannelConfig>,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChannelConfig {
    QQ {},
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    OpenAI {
        api_key: String,
    },
    OpenAICodex {
        access_token: String,
        account_id: Option<String>,
        model: String,
        base_url: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn existing_openai_provider_config_still_deserializes() {
        let config: ProviderConfig = serde_json::from_value(json!({
            "type": "OpenAI",
            "api_key": "sk-test",
        }))
        .unwrap();

        assert!(matches!(config, ProviderConfig::OpenAI { api_key } if api_key == "sk-test"));
    }

    #[test]
    fn openai_codex_provider_config_deserializes_with_access_token() {
        let config: ProviderConfig = serde_json::from_value(json!({
            "type": "OpenAICodex",
            "access_token": "token",
            "account_id": "acct",
            "model": "gpt-5.2-codex",
            "base_url": "https://example.test/backend-api/codex",
        }))
        .unwrap();

        match config {
            ProviderConfig::OpenAICodex {
                access_token,
                account_id,
                model,
                base_url,
            } => {
                assert_eq!(access_token, "token");
                assert_eq!(account_id.as_deref(), Some("acct"));
                assert_eq!(model, "gpt-5.2-codex");
                assert_eq!(
                    base_url.as_deref(),
                    Some("https://example.test/backend-api/codex")
                );
            }
            ProviderConfig::OpenAI { .. } => panic!("expected OpenAICodex config"),
        }
    }

    #[test]
    fn openai_codex_provider_config_requires_model() {
        let result = serde_json::from_value::<ProviderConfig>(json!({
            "type": "OpenAICodex",
            "access_token": "token",
        }));

        assert!(result.is_err());
    }
}
