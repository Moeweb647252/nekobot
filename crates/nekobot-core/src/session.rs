use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use turso::Connection;

use crate::entity::{self, message::Message};

static MESSAGE_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
        Message::create(
            &self.app_db,
            next_message_id(self.session_id),
            self.session_id,
            role,
            content,
            reasoning_content,
        )
        .await
    }
}

fn next_message_id(session_id: i64) -> String {
    let sequence = MESSAGE_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    format!("session-{session_id}-{timestamp}-{sequence}")
}
