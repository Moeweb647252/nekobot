use std::sync::Arc;
use std::{num::NonZero, ops::Deref};

use redis::{aio::MultiplexedConnection, AsyncCommands, RedisError};

#[derive(Clone)]
pub struct Db {
  pub conn: MultiplexedConnection,
}

impl From<Arc<MultiplexedConnection>> for Db {
  fn from(conn: Arc<MultiplexedConnection>) -> Self {
    Self {
      conn: conn.deref().clone(),
    }
  }
}

type Result<T> = std::result::Result<T, RedisError>;

impl Db {
  pub async fn check_user(&mut self, user_id: u64) -> Result<bool> {
    self
      .conn
      .hexists(format!("users:{}", user_id), "enabled")
      .await
  }

  pub async fn enable_user(&mut self, user_id: u64) -> Result<()> {
    redis::cmd("HSET")
      .arg(format!("users:{}", user_id))
      .arg("enabled")
      .arg(true)
      .exec_async(&mut self.conn)
      .await
  }

  pub async fn get_messages(&mut self, chat_id: i64) -> Result<Vec<String>> {
    self
      .conn
      .lrange(format!("messages:{}", chat_id), 0, -1)
      .await
  }

  pub async fn add_message(&mut self, chat_id: i64, message: String) -> Result<()> {
    redis::cmd("RPUSH")
      .arg(format!("messages:{}", chat_id))
      .arg(message)
      .exec_async(&mut self.conn)
      .await
  }

  pub async fn pop_message(&mut self, chat_id: i64) -> Result<Option<String>> {
    self
      .conn
      .rpop(
        format!("messages:{}", chat_id),
        Some(NonZero::new(1).unwrap()),
      )
      .await
  }

  pub async fn increase_token(
    &mut self,
    user_id: u64,
    completion_tokens: usize,
    prompt_tokens: usize,
  ) -> Result<()> {
    redis::cmd("HINCRBY")
      .arg(format!("users:{}", user_id))
      .arg("completion_tokens")
      .arg(completion_tokens)
      .exec_async(&mut self.conn)
      .await?;
    redis::cmd("HINCRBY")
      .arg(format!("users:{}", user_id))
      .arg("prompt_tokens")
      .arg(prompt_tokens)
      .exec_async(&mut self.conn)
      .await
  }
}
