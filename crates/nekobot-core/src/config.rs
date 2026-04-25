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
    OpenAI { api_key: String },
}
