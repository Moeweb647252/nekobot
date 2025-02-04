use std::ops::Deref;
use std::sync::Arc;

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
    self.conn.lpop(format!("messages:{}", chat_id), None).await
  }
}
