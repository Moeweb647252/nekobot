#![allow(dead_code)]

use anyhow::Context;
use log::{debug, info};
use reqwest::Proxy;
use serde::Serialize;

use crate::CONFIG;

use super::{Message, TextProvider};

#[derive(Clone)]
pub struct OpenAI {
  api_key: String,
  api_base: String,
  model: String,
  _reqwest: reqwest::Client,
}

#[derive(serde::Serialize, Default, Debug)]
pub struct Payload {
  pub model: String,
  pub messages: Vec<PayloadMessage>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub frequency_penalty: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub max_tokens: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub n: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub response_format: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub stop: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub temperature: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub top_p: Option<f32>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub stream: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub stream_options: Option<PayloadStreamOptions>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub top_k: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub logprobs: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub top_logprobs: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub seed: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tools: Option<PayloadTools>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tool_choice: Option<String>,
}

#[derive(serde::Serialize, Debug)]
pub struct PayloadTools {
  #[serde(rename = "type")]
  pub _type: ToolCallsType,
  pub function: Vec<PayloadToolCallFunction>,
}

#[derive(serde::Serialize, Debug)]
pub struct PayloadToolCallFunction {
  pub name: String,
  pub parameters: Option<String>,
  pub description: Option<String>,
  pub strict: Option<bool>,
}

#[derive(serde::Serialize, Debug)]
pub struct PayloadMessage {
  pub role: String,
  pub content: String,
}

#[derive(serde::Serialize, Debug)]
pub struct PayloadStreamOptions {
  pub include_usage: bool,
}

#[derive(serde::Deserialize, Debug)]
pub struct ResponseMessage {
  pub role: String,
  pub content: String,
  pub resoning_content: Option<String>,
  pub tool_calls: Option<Vec<ResponseToolCalls>>,
}

#[derive(serde::Deserialize, Debug)]
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

#[derive(serde::Deserialize, Debug)]
pub struct Choice {
  pub message: ResponseMessage,
}

#[derive(serde::Deserialize, Debug)]
pub struct Usage {
  pub completion_tokens: i64,
  pub prompt_tokens: i64,
  pub total_tokens: i64,
}

#[derive(serde::Deserialize, Serialize, Debug)]
pub enum ToolCallsType {
  #[serde(rename = "function")]
  Function,
}

#[derive(serde::Deserialize, Debug)]
pub struct ResponseToolCalls {
  pub id: String,
  #[serde(rename = "type")]
  pub _type: ToolCallsType,
}

#[derive(serde::Deserialize, Debug)]
pub struct ResponseToolCallFunction {
  name: String,
  arguments: String,
}

#[derive(serde::Deserialize, Debug)]
pub struct Response {
  choices: Vec<Choice>,
  created: i64,
  id: String,
  model: String,
  object: Option<String>,
  usage: Option<Usage>,
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
  async fn completion(&self, msg: Vec<Message>) -> anyhow::Result<(String, Option<super::Usage>)> {
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
    debug!("Payload: {}", serde_json::to_string_pretty(&payload)?);
    let response = reqwest::Client::new()
      .post(format!("{}/chat/completions", self.api_base))
      .header("Authorization", format!("Bearer {}", self.api_key))
      .json(&payload)
      .send()
      .await?;

    debug!("Response status: {}", response.status());
    let text = response.text().await?;
    debug!("Response text: {}", text.as_str());
    let response = serde_json::from_str::<Response>(text.as_str())?;
    if let Some(usage) = &response.usage {
      info!(
        "Tokens: {}, Prompt tokens: {}, Total tokens: {}",
        usage.completion_tokens, usage.prompt_tokens, usage.total_tokens
      );
    }
    let usage = response.usage.map(|usage| super::Usage {
      completion: usage.completion_tokens as usize,
      prompt: usage.prompt_tokens as usize,
    });
    Ok((
      response
        .choices
        .first()
        .context("Invalid response")?
        .message
        .content
        .to_string(),
      usage,
    ))
  }
}
