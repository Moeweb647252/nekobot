use crate::db;
use crate::CompletionTask;
use crate::CONFIG;
use async_openai::config::OpenAIConfig;
use async_openai::types::ChatCompletionRequestAssistantMessageArgs;
use async_openai::types::ChatCompletionRequestSystemMessageArgs;
use async_openai::types::ChatCompletionRequestUserMessageArgs;
use log::debug;
use tokio::sync::mpsc;

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

async fn handel_completion(
  msg: &CompletionTask,
  mut db: db::Db,
  openai: async_openai::Client<OpenAIConfig>,
) -> anyhow::Result<String> {
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
  messages.append(
    &mut (messages_history
      .into_iter()
      .map(|m| {
        let message = parse_message(&m);
        match message {
          Message::User(text) => ChatCompletionRequestUserMessageArgs::default()
            .content(text)
            .build()
            .unwrap()
            .into(),
          Message::Assitant(text) => ChatCompletionRequestAssistantMessageArgs::default()
            .content(text)
            .build()
            .unwrap()
            .into(),
        }
      })
      .collect::<Vec<_>>()),
  );
  match msg.msg.as_str() {
    "/retry" => (),
    "/regenerate" => {
      messages.pop();
      db.pop_message(msg.chat_id).await?;
    }
    _ => {
      messages.push(
        ChatCompletionRequestUserMessageArgs::default()
          .content(msg.msg.as_str())
          .build()
          .unwrap()
          .into(),
      );
      db.add_message(msg.chat_id, format!("USER:{}", msg.msg.as_str()))
        .await?;
    }
  }
  let openai = openai.clone();
  let mut db = db.clone();

  let request = async_openai::types::CreateChatCompletionRequestArgs::default()
    .model(CONFIG.llm_model.as_str())
    .messages(messages)
    .build()
    .unwrap();
  debug!("Sending request: {:?}", request);
  let response = openai.chat().create(request).await.unwrap();
  if let Some(response) = response.choices.first() {
    let resp = response.message.content.clone().unwrap_or_default();
    db.add_message(msg.chat_id, format!("ASSISTANT:{}", resp.as_str()))
      .await?;
    Ok(resp)
  } else {
    Err(anyhow::anyhow!("No response from LLM provider"))
  }
}

pub fn openai_task(mut rx: mpsc::UnboundedReceiver<crate::CompletionTask>, db: db::Db) {
  tokio::spawn(async move {
    let openai = async_openai::Client::with_config(
      async_openai::config::OpenAIConfig::new()
        .with_api_base(CONFIG.llm_api_base.as_str())
        .with_api_key(CONFIG.llm_api_key.as_str()),
    );
    while let Some(msg) = rx.recv().await {
      let openai = openai.clone();
      let db = db.clone();
      tokio::spawn(async move {
        let res = handel_completion(&msg, db, openai).await;
        if let Err(e) = msg.sender.send(res) {
          log::error!("Failed to send response: {:?}", e);
        }
      });
    }
  });
}
