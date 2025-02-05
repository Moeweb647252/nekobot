use serde::Deserialize;

#[derive(Deserialize)]
pub struct Text {
  pub llm_api_key: String,
  pub llm_api_base: String,
  pub llm_model: String,
}

#[derive(Deserialize)]
pub struct Image {
  pub llm_api_key: String,
  pub llm_api_base: String,
  pub llm_model: String,
  pub provider: String,
}

#[derive(Deserialize)]
pub struct Config {
  pub text: Text,
  pub image: Option<Image>,
  pub redis_url: String,
  pub bot_token: String,
  pub system_prompt: String,
  pub password: String,
  pub context_length: usize,
  pub log_level: String,
}

impl Config {
  pub fn from_file(path: &str) -> Self {
    toml::from_str(std::fs::read_to_string(path).unwrap().as_str()).unwrap()
  }
}
