use serde::Deserialize;

use crate::providers::{DynTextProvider, OpenAI};

#[derive(Deserialize)]
pub struct Text {
  pub api_key: String,
  pub api_base: String,
  pub model: String,
  pub provider: String,
  pub temperature: Option<f32>,
  pub top_p: Option<f32>,
  pub max_tokens: Option<usize>,
}

#[derive(Deserialize)]
pub struct Image {
  pub api_key: String,
  pub api_base: String,
  pub model: String,
  pub provider: String,
}

#[derive(Deserialize)]
pub struct Bot {
  pub token: String,
  pub proxy: Option<String>,
}

#[derive(Deserialize)]
pub struct Config {
  pub text: Text,
  pub image: Option<Image>,
  pub redis_url: String,
  pub bot: Bot,
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

impl Text {
  pub fn make_provider(&self) -> Box<DynTextProvider> {
    match self.provider.to_lowercase().as_str() {
      "openai" => DynTextProvider::boxed(
        OpenAI::new(
          self.api_key.clone(),
          self.api_base.clone(),
          self.model.clone(),
          None,
        )
        .unwrap(),
      ),
      _ => panic!("Invalid provider"),
    }
  }
}

impl Bot {
  pub fn make_bot(&self) -> teloxide::Bot {
    let client = reqwest::ClientBuilder::new();
    let client = if let Some(proxy) = &self.proxy {
      client.proxy(reqwest::Proxy::all(proxy).unwrap())
    } else {
      client
    };
    teloxide::Bot::with_client(self.token.as_str(), client.build().unwrap())
  }
}
