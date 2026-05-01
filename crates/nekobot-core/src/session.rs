//! In-memory session handle used during message processing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use turso::Connection;

use crate::entity::{self, message::Message};

static MESSAGE_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// A handle to an active session, combining a database connection and session ID.
///
/// Used by [`AgentSession`](crate::agent::AgentSession) to persist messages.
pub struct Session {
    pub session_id: i64,
    pub app_db: Connection,
}

impl Session {
    /// Persist a chat message to the database.
    ///
    /// Generates a unique message ID from timestamp + atomic sequence.
    pub async fn add_message(
        &self,
        role: impl Into<String>,
        content: impl Into<String>,
        reasoning_content: Option<String>,
        tool_call_id: Option<String>,
    ) -> anyhow::Result<entity::message::Message> {
        Message::create(
            &self.app_db,
            next_message_id(self.session_id),
            self.session_id,
            role,
            content,
            reasoning_content,
            tool_call_id,
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
