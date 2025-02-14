#![allow(dead_code)]
use serde::Deserialize;

use crate::providers::{DynTextToTextProvider, OpenAI};

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
pub struct OpenRouter {
  pub order: Vec<String>,
  pub allow_fallback: Option<bool>,
  pub sort: Option<String>,
  pub quantizations: Option<Vec<String>>,
  pub allow_fallbacks: Option<bool>,
  pub ignore: Option<Vec<String>>,
  pub require_parameters: Option<bool>,
}

#[derive(Deserialize)]
pub struct Config {
  pub text: Text,
  pub image: Option<Image>,
  pub enable_msg: String,
  pub start_msg: String,
  pub reset_msg: String,
  pub error_msg: String,
  pub redis_url: String,
  pub bot: Bot,
  pub system_prompt: String,
  pub password: String,
  pub context_length: usize,
  pub log_level: String,
  #[serde(default)]
  pub concurrency: usize,
  pub queuing_msg: Option<String>,
  pub open_router: Option<OpenRouter>,
}

impl Config {
  pub fn from_file(path: &str) -> Self {
    toml::from_str(std::fs::read_to_string(path).unwrap().as_str()).unwrap()
  }
}

impl Text {
  pub fn make_provider(&self) -> Box<DynTextToTextProvider> {
    match self.provider.to_lowercase().as_str() {
      "openai" => DynTextToTextProvider::boxed(
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
