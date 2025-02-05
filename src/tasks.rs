use crate::db;
use crate::CONFIG;
use async_openai::config::OpenAIConfig;
use async_openai::types::ChatCompletionRequestAssistantMessageArgs;
use async_openai::types::ChatCompletionRequestSystemMessageArgs;
use async_openai::types::ChatCompletionRequestUserMessageArgs;
use log::debug;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub enum CompletionType {
  Text { msg: String },
  Image { data: Vec<u8> },
}

pub struct CompletionTask {
  pub chat_id: i64,
  pub data: CompletionType,
  pub sender: oneshot::Sender<anyhow::Result<String>>,
}

enum Message<'a> {
  User(&'a str),
  Assitant(&'a str),
}

fn parse_message(msg: &str) -> Message {
  if msg.starts_with("USER:") {
    Message::User(msg.strip_prefix("USER:").unwrap())
  } else {
    Message::Assitant(msg.strip_prefix("ASSISTANT:").unwrap())
  }
}

async fn handel_text_completion(
  msg: &CompletionTask,
  mut db: db::Db,
  openai: async_openai::Client<OpenAIConfig>,
) -> anyhow::Result<String> {
  let to_send = match &msg.data {
    CompletionType::Text { msg } => msg,
    _ => return Err(anyhow::anyhow!("Invalid data type")),
  };
  debug!("Starting task for {}", msg.chat_id);
  let mut messages = vec![ChatCompletionRequestSystemMessageArgs::default()
    .content(CONFIG.system_prompt.as_str())
    .build()?
    .into()];
  let mut messages_history = db.get_messages(msg.chat_id).await?;
  if messages_history.len() > CONFIG.context_length {
    messages_history = messages_history
      .split_at(messages_history.len() - CONFIG.context_length)
      .1
      .to_vec();
  }
  for i in messages_history.into_iter() {
    let message = parse_message(&i);
    messages.push(match message {
      Message::User(text) => ChatCompletionRequestUserMessageArgs::default()
        .content(text)
        .build()?
        .into(),
      Message::Assitant(text) => ChatCompletionRequestAssistantMessageArgs::default()
        .content(text)
        .build()?
        .into(),
    });
  }
  match to_send.as_str() {
    "/retry" => (),
    "/regenerate" => {
      messages.pop();
      db.pop_message(msg.chat_id).await?;
    }
    _ => {
      messages.push(
        ChatCompletionRequestUserMessageArgs::default()
          .content(to_send.as_str())
          .build()?
          .into(),
      );
      db.add_message(msg.chat_id, format!("USER:{}", to_send.as_str()))
        .await?;
    }
  }
  let openai = openai.clone();
  let mut db = db.clone();

  let request = async_openai::types::CreateChatCompletionRequestArgs::default()
    .model(CONFIG.text.llm_model.as_str())
    .messages(messages)
    .build()?;
  debug!("Sending request: {:?}", request);
  let response = openai.chat().create(request).await?;
  if let Some(response) = response.choices.first() {
    let resp = response.message.content.clone().unwrap_or_default();
    db.add_message(msg.chat_id, format!("ASSISTANT:{}", resp.as_str()))
      .await?;
    Ok(resp)
  } else {
    Err(anyhow::anyhow!("No response from LLM provider"))
  }
}

async fn handel_image_completion(
  _msg: &CompletionTask,
  _db: db::Db,
  _openai: async_openai::Client<OpenAIConfig>,
) -> anyhow::Result<String> {
  todo!()
}

pub fn ai_task(mut rx: mpsc::UnboundedReceiver<CompletionTask>, db: db::Db) {
  tokio::spawn(async move {
    let openai = async_openai::Client::with_config(
      async_openai::config::OpenAIConfig::new()
        .with_api_base(CONFIG.text.llm_api_base.as_str())
        .with_api_key(CONFIG.text.llm_api_key.as_str()),
    );
    while let Some(msg) = rx.recv().await {
      let openai = openai.clone();
      let db = db.clone();
      tokio::spawn(async move {
        let res = handel_text_completion(&msg, db, openai).await;
        if let Err(e) = msg.sender.send(res) {
          log::error!("Failed to send response: {:?}", e);
        }
      });
    }
  });
}
