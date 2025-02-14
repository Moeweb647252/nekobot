#![allow(dead_code)]
#![allow(unused_imports)]

use crate::config;
use crate::db;
use crate::providers::Message;
use crate::providers::TextToTextProvider;
use crate::providers::{DynImageToTextProvider, DynTextToTextProvider};
use crate::CONFIG;
use anyhow::Context;
use log::debug;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub enum CompletionType {
  Text { msg: String },
  Image { data: Vec<u8> },
}

pub struct CompletionTask {
  pub chat_id: i64,
  pub user_id: u64,
  pub data: CompletionType,
  pub sender: oneshot::Sender<anyhow::Result<String>>,
}

fn parse_message(msg: &str) -> anyhow::Result<Message> {
  Ok(if msg.starts_with("USER:") {
    Message::User(
      msg
        .strip_prefix("USER:")
        .context("Invaild message")?
        .to_string(),
    )
  } else {
    Message::Assitant(
      msg
        .strip_prefix("ASSISTANT:")
        .context("Invaild message")?
        .to_string(),
    )
  })
}

async fn handel_text_completion(
  msg: &CompletionTask,
  mut db: db::Db,
  provider: Arc<DynTextToTextProvider<'_>>,
) -> anyhow::Result<String> {
  let to_send = match &msg.data {
    CompletionType::Text { msg } => msg,
    _ => return Err(anyhow::anyhow!("Invalid data type")),
  };
  debug!("Starting task for {}", msg.chat_id);
  let mut messages = vec![Message::System(CONFIG.system_prompt.to_string())];
  let mut messages_history = db.get_messages(msg.chat_id).await?;
  if messages_history.len() > CONFIG.context_length {
    messages_history = messages_history
      .split_at(messages_history.len() - CONFIG.context_length)
      .1
      .to_vec();
  }
  for i in messages_history.into_iter() {
    let message = parse_message(&i)?;
    messages.push(message);
  }
  match to_send.as_str() {
    "/retry" => (),
    "/regenerate" => {
      messages.pop();
      db.pop_message(msg.chat_id).await?;
    }
    "/reset" => {
      db.clear_messages(msg.chat_id).await?;
      return Ok(CONFIG.reset_msg.clone());
    }
    _ => {
      messages.push(Message::User(to_send.to_string()));
      db.add_message(msg.chat_id, format!("USER:{}", to_send.as_str()))
        .await?;
    }
  }
  let mut db = db.clone();
  let (response, usage) = provider.completion(messages).await?;
  if let Some(usage) = usage {
    db.increase_token(msg.user_id, usage.completion, usage.prompt)
      .await?;
  }
  db.add_message(msg.chat_id, format!("ASSISTANT:{}", response.as_str()))
    .await?;
  Ok(response)
}

pub fn ai_task(mut rx: mpsc::UnboundedReceiver<CompletionTask>, db: db::Db) {
  tokio::spawn(async move {
    let text_provider: Arc<DynTextToTextProvider> = Arc::from(CONFIG.text.make_provider());
    while let Some(msg) = rx.recv().await {
      let text_provider = text_provider.clone();
      let db = db.clone();
      tokio::spawn(async move {
        let res = handel_text_completion(&msg, db, text_provider).await;
        if let Err(e) = msg.sender.send(res) {
          log::error!("Failed to send response: {:?}", e);
        }
      });
    }
  });
}
