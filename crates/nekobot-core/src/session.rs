//! In-memory session handle used during message processing.

use turso::Connection;

use crate::entity::{self, message::Message};

/// Tool call metadata for persisting assistant/tool messages.
pub struct ToolPayload {
    /// Tool call ID, used when role is "tool".
    pub tool_call_id: String,
    /// Serialized tool calls JSON, used when role is "assistant".
    pub tool_calls: Option<String>,
}

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
        tool: Option<ToolPayload>,
    ) -> anyhow::Result<entity::message::Message> {
        Message::create(
            &self.app_db,
            self.session_id,
            role,
            content,
            reasoning_content,
            tool.as_ref().map(|t| t.tool_call_id.clone()),
            tool.as_ref().and_then(|t| t.tool_calls.clone()),
        )
        .await
    }
}
