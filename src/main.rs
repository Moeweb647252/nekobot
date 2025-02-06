use clap::Parser;
use db::Db;
use log::info;
use redis::aio::MultiplexedConnection;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::{ops::Deref, sync::Arc};
use tasks::{ai_task, CompletionTask};
use teloxide::types::BotCommand;
use teloxide::{dispatching::dialogue::GetChatId, prelude::*};
use tokio::sync::mpsc;

mod config;
mod db;
mod providers;
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

static QUEUE_SIZE: AtomicUsize = AtomicUsize::new(0);

#[tokio::main]
async fn main() {
  std::env::set_var("RUST_LOG", CONFIG.log_level.as_str());
  pretty_env_logger::init();
  log::info!("Starting nekobot...");

  let bot = CONFIG.bot.make_bot();
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
  ai_task(rx, db);
  let handler = Update::filter_message().endpoint(
    |bot: Bot,
     conn: Arc<MultiplexedConnection>,
     tx: Arc<mpsc::UnboundedSender<CompletionTask>>,
     msg: teloxide::prelude::Message| async move {
      let mut db = Db::from(conn);
      let tx = tx.deref().clone();
      log::debug!("Received message: {:?}", msg);
      log::info!("Received message from: {:?}", msg.from);
      if let Some(user) = &msg.from {
        if let Some(chat_id) = msg.chat_id() {
          if db.check_user(user.id.0).await.unwrap() {
            log::info!("Responding to {}", user.id.0);
            if QUEUE_SIZE.load(Ordering::Acquire) < CONFIG.concurrency || CONFIG.concurrency == 0 {
              QUEUE_SIZE.fetch_add(1, Ordering::AcqRel);
              responder::user_respond(bot, tx, user.id.0, msg, chat_id.0).await;
              QUEUE_SIZE.fetch_sub(1, Ordering::AcqRel);
            } else {
              bot
                .send_message(
                  chat_id,
                  CONFIG
                    .queuing_msg
                    .as_ref()
                    .map(|v| &v[..])
                    .unwrap_or("Queue is full, please try again later."),
                )
                .await?;
            }
          } else if msg.text().unwrap_or("").eq(CONFIG.password.as_str()) {
            info!("User {} enabled", user.id.0);
            db.enable_user(user.id.0).await.unwrap();
            bot
              .send_message(chat_id, CONFIG.enable_msg.as_str())
              .await?;
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
