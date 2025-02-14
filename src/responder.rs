use std::io::Cursor;

use bytes::{BufMut, BytesMut};
use log::error;
use teloxide::net::Download;
use teloxide::types::{ChatAction, PhotoSize};
use teloxide::{
  prelude::{Request, Requester},
  types::{ChatId, Message},
  Bot,
};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::tasks::{CompletionTask, CompletionType};
use crate::CONFIG;

async fn image_process(
  bot: Bot,
  photo: PhotoSize,
  tx: mpsc::UnboundedSender<CompletionTask>,
) -> anyhow::Result<String> {
  let file = bot.get_file(photo.file.id).await?;
  let mut buf = Cursor::new(Vec::new());
  bot.download_file(&file.path, &mut buf).await?;
  let buf = buf.into_inner();

  todo!()
}

pub async fn user_respond(
  bot: Bot,
  tx: mpsc::UnboundedSender<CompletionTask>,
  user_id: u64,
  msg: Message,
  chat_id: i64,
) {
  // 发送初始打字状态
  if let Err(e) = bot
    .send_chat_action(ChatId(chat_id), ChatAction::Typing)
    .await
  {
    error!(
      "Failed to send typing chat action (chat_id: {}): {:?}",
      chat_id, e
    );
    return;
  }

  let (sender, mut receiver) = tokio::sync::oneshot::channel();
  if let Some(images) = msg.photo() {
    for image in images.into_iter() {}
  }
  let to_send = if let Some(reply_to) = msg.reply_to_message() {
    format!(
      "Reply to: ```\n{}\n```\n{}",
      reply_to.text().unwrap_or(""),
      msg.text().unwrap_or("")
    )
  } else {
    msg.text().unwrap_or("").to_string()
  };
  let task = CompletionTask {
    chat_id,
    user_id,
    data: CompletionType::Text { msg: to_send },
    sender,
  };

  if let Err(e) = tx.send(task) {
    error!("Channel send failed (chat_id: {}): {:?}", chat_id, e);
    return;
  }

  let sleep = tokio::time::sleep(tokio::time::Duration::from_secs(4));
  tokio::pin!(sleep);
  let response = loop {
    tokio::select! {
        _ = &mut sleep => {
            if let Err(e) = bot.send_chat_action(ChatId(chat_id), ChatAction::Typing).await {
                error!("Recurring chat action failed (chat_id: {}): {:?}", chat_id, e);
            }
            sleep.as_mut().reset(Instant::now() + tokio::time::Duration::from_secs(4));
        }
        resp = &mut receiver => {
            break match resp {
                Ok(r) => r,
                Err(e) => {
                    error!("Response channel closed unexpectedly (chat_id: {}): {:?}", chat_id, e);
                    break Ok(CONFIG.error_msg.clone());
                }
            };
        }
    }
  };
  match response {
    Ok(response) => {
      if let Err(e) = bot.send_message(ChatId(chat_id), response).send().await {
        error!("Final message send failed (chat_id: {}): {:?}", chat_id, e);
      }
    }
    Err(e) => {
      bot
        .send_message(ChatId(chat_id), CONFIG.error_msg.as_str())
        .send()
        .await
        .ok();
      error!("Response generation failed (chat_id: {}): {:?}", chat_id, e);
    }
  }
}
