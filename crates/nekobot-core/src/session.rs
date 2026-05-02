//! In-memory session handle used during message processing.

use turso::Connection;

use crate::entity::{self, message::Message};

/// A handle to an active session, combining a database connection and session ID.
///
/// Used by [`AgentSession`](crate::agent::AgentSession) to persist messages.
pub struct SessionHandle {
    pub session_id: i64,
    pub app_db: Connection,
}

impl SessionHandle {
    /// Persist a chat message to the database.
    pub async fn add_message(
        &self,
        role: impl Into<String>,
        content: impl Into<String>,
        reasoning_content: Option<String>,
        tool_call_id: Option<String>,
    ) -> anyhow::Result<entity::message::Message> {
        Message::create(
            &self.app_db,
            self.session_id,
            role,
            content,
            reasoning_content,
            tool_call_id,
        )
        .await
    }
}
