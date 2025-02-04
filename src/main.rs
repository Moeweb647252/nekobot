use clap::Parser;
use db::Db;
use log::info;
use redis::aio::MultiplexedConnection;
use std::sync::LazyLock;
use std::{ops::Deref, sync::Arc};
use tasks::openai_task;
use teloxide::types::BotCommand;
use teloxide::{dispatching::dialogue::GetChatId, prelude::*};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

mod config;
mod db;
mod responder;
mod tasks;

/// Simple program to greet a person
#[derive(Parser)]
#[command(name = "NekoBot", about = "NekoBot", version = "1.0", long_about = "")]
pub struct Args {
  /// Path to the configuration file
  #[arg(
    help = "Path to the configuration file",
    required = true,
    value_name = "CONFIG_FILE",
    short = 'c'
  )]
  pub config: String,
}

static CONFIG: LazyLock<config::Config> = LazyLock::new(|| {
  let args = Args::parse();
  config::Config::from_file(&args.config)
});

struct CompletionTask {
  chat_id: i64,
  msg: String,
  sender: oneshot::Sender<anyhow::Result<String>>,
}

#[tokio::main]
async fn main() {
  std::env::set_var("RUST_LOG", CONFIG.log_level.as_str());
  pretty_env_logger::init();
  log::info!("Starting nekobot...");

  let bot = Bot::new(CONFIG.bot_token.as_str());
  bot
    .set_my_commands(vec![
      BotCommand {
        command: "retry".to_owned(),
        description: "Retry to generate a response".to_owned(),
      },
      BotCommand {
        command: "regenerate".to_owned(),
        description: "Regenerate last message".to_owned(),
      },
    ])
    .await
    .unwrap();
  let redis = redis::Client::open(CONFIG.redis_url.as_str()).unwrap();
  let conn = redis.get_multiplexed_async_connection().await.unwrap();
  let (tx, rx) = mpsc::unbounded_channel();
  let db = Db { conn: conn.clone() };
  openai_task(rx, db);
  let handler = Update::filter_message().endpoint(
    |bot: Bot,
     conn: Arc<MultiplexedConnection>,
     tx: Arc<mpsc::UnboundedSender<CompletionTask>>,
     msg: teloxide::prelude::Message| async move {
      let mut db = Db::from(conn);
      let tx = tx.deref().clone();
      log::info!("Received message: {:?}", msg);
      if let Some(user) = &msg.from {
        if let Some(chat_id) = msg.chat_id() {
          if db.check_user(user.id.0).await.unwrap() {
            log::info!("Responding to {}", user.id.0);
            responder::user_respond(bot, tx, user.id.0, msg, chat_id.0).await
          } else if msg.text().unwrap_or("").eq(CONFIG.password.as_str()) {
            info!("User {} enabled", user.id.0);
            db.enable_user(user.id.0).await.unwrap();
            bot.send_message(chat_id, "猫猫激活成功!").await.unwrap();
          }
        }
      }
      respond(())
    },
  );

  Dispatcher::builder(bot, handler)
    // Pass the shared state to the handler as a dependency.
    .dependencies(dptree::deps![Arc::new(conn), Arc::new(tx)])
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await;
}
