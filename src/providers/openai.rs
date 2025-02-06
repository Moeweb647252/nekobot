#![allow(dead_code)]

use log::debug;
use reqwest::Proxy;
use serde::Serialize;
use serde_json::Value;

use crate::CONFIG;

use super::{Message, TextProvider};

#[derive(Clone)]
pub struct OpenAI {
  api_key: String,
  api_base: String,
  model: String,
  _reqwest: reqwest::Client,
}

#[derive(serde::Serialize, Default)]
pub struct Payload {
  pub model: String,
  pub messages: Vec<PayloadMessage>,
  pub frequency_penalty: Option<f64>,
  pub max_tokens: Option<usize>,
  pub n: Option<usize>,
  pub response_format: Option<String>,
  pub stop: Option<Vec<String>>,
  pub temperature: Option<f32>,
  pub top_p: Option<f32>,
  pub stream: Option<bool>,
  pub stream_options: Option<PayloadStreamOptions>,
  pub top_k: Option<i64>,
  pub logprobs: Option<bool>,
  pub top_logprobs: Option<i64>,
  pub seed: Option<i64>,
  pub tools: Option<PayloadTools>,
  pub tool_choice: Option<String>,
}

#[derive(serde::Serialize)]
pub struct PayloadTools {
  #[serde(rename = "type")]
  pub _type: ToolCallsType,
  pub function: Vec<PayloadToolCallFunction>,
}

#[derive(serde::Serialize)]
pub struct PayloadToolCallFunction {
  pub name: String,
  pub parameters: Option<String>,
  pub description: Option<String>,
  pub strict: Option<bool>,
}

#[derive(serde::Serialize)]
pub struct PayloadMessage {
  pub role: String,
  pub content: String,
}

#[derive(serde::Serialize)]
pub struct PayloadStreamOptions {
  pub include_usage: bool,
}

#[derive(serde::Deserialize)]
pub struct ResponseMessage {
  pub role: String,
  pub content: String,
  pub resoning_content: Option<String>,
  pub tool_calls: Option<Vec<ResponseToolCalls>>,
}

#[derive(serde::Deserialize)]
pub enum FinishReason {
  #[serde(rename = "stop")]
  Stop,
  #[serde(rename = "eos")]
  Eos,
  #[serde(rename = "length")]
  Length,
  #[serde(rename = "tool_calls")]
  ToolCalls,
}

#[derive(serde::Deserialize)]
pub struct Choice {
  pub message: ResponseMessage,
}

#[derive(serde::Deserialize)]
pub struct Usage {
  pub completion_tokens: i64,
  pub prompt_tokens: i64,
  pub total_tokens: i64,
}

#[derive(serde::Deserialize, Serialize)]
pub enum ToolCallsType {
  #[serde(rename = "function")]
  Function,
}

#[derive(serde::Deserialize)]
pub struct ResponseToolCalls {
  pub id: String,
  #[serde(rename = "type")]
  pub _type: ToolCallsType,
}

#[derive(serde::Deserialize)]
pub struct ResponseToolCallFunction {
  name: String,
  arguments: String,
}

#[derive(serde::Deserialize)]
pub struct Response {
  choices: Vec<Choice>,
  created: i64,
  id: String,
  model: String,
  object: String,
  usage: Usage,
}

impl OpenAI {
  pub fn new(
    api_key: String,
    api_base: String,
    model: String,
    proxy: Option<String>,
  ) -> anyhow::Result<Self> {
    let client = reqwest::Client::builder();
    let client = if let Some(proxy) = proxy {
      client.proxy(Proxy::all(proxy)?)
    } else {
      client
    };
    let client = client.build()?;
    Ok(Self {
      api_key,
      api_base,
      model,
      _reqwest: client,
    })
  }
}

impl TextProvider for OpenAI {
  async fn completion(&self, msg: Vec<Message>) -> anyhow::Result<String> {
    let messages = msg
      .into_iter()
      .map(|msg| match msg {
        Message::User(text) => PayloadMessage {
          role: "user".to_string(),
          content: text,
        },
        Message::Assitant(text) => PayloadMessage {
          role: "assistant".to_string(),
          content: text,
        },
        Message::System(text) => PayloadMessage {
          role: "system".to_string(),
          content: text,
        },
      })
      .collect();
    let payload = Payload {
      model: self.model.clone(),
      messages,
      top_p: CONFIG.text.top_p,
      max_tokens: CONFIG.text.max_tokens,
      temperature: CONFIG.text.temperature,
      ..Default::default()
    };
    let response = reqwest::Client::new()
      .post(format!("{}/chat/completions", self.api_base))
      .header("Authorization", format!("Bearer {}", self.api_key))
      .json(&payload)
      .send()
      .await?
      .text()
      .await?;
    debug!("Response: {}", &response);
    Ok(
      serde_json::from_str::<Value>(&response)?["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string(),
    )
  }
}
