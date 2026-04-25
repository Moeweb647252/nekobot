use turso::Connection;

use crate::entity;

pub struct Session {
    pub session_id: i64,
    pub app_db: Connection,
}

impl Session {
    pub async fn add_message(
        &self,
        role: impl Into<String>,
        content: impl Into<String>,
        reasoning_content: Option<String>,
    ) -> anyhow::Result<entity::message::Message> {
        todo!()
    }
}
