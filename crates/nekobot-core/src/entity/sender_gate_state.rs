//! Sender gate state entity — persistent login/agent-binding state for C2C access control.

use turso::Connection;

use crate::entity::Entity;

/// Persistent login and agent-binding state for a sender within a channel.
///
/// Composite primary key is `(channel_id, sender_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderGateState {
    pub channel_id: String,
    pub sender_id: String,
    pub is_logged_in: bool,
    pub connected_agent: Option<String>,
}

impl SenderGateState {
    /// Create a new default (unlogged) state for the given sender.
    pub fn new(channel_id: impl Into<String>, sender_id: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            sender_id: sender_id.into(),
            is_logged_in: false,
            connected_agent: None,
        }
    }

    fn from_row(row: &turso::Row) -> anyhow::Result<Self> {
        let connected_agent: Option<String> = row.get(3)?;
        Ok(Self {
            channel_id: row.get(0)?,
            sender_id: row.get(1)?,
            is_logged_in: {
                let v: i64 = row.get(2)?;
                v != 0
            },
            connected_agent,
        })
    }

    /// Look up the gate state for a sender, or return `None` if not found.
    pub async fn get(
        conn: &Connection,
        channel_id: &str,
        sender_id: &str,
    ) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT channel_id, sender_id, is_logged_in, connected_agent
                 FROM sender_gate_states
                 WHERE channel_id = ?1 AND sender_id = ?2",
                (channel_id, sender_id),
            )
            .await?;

        rows.next().await?.map(|row| Self::from_row(&row)).transpose()
    }

    /// Insert or replace the gate state.
    pub async fn upsert(&self, conn: &Connection) -> anyhow::Result<Self> {
        let connected_agent = self.connected_agent.as_deref().unwrap_or("");
        conn.execute(
            "INSERT OR REPLACE INTO sender_gate_states (channel_id, sender_id, is_logged_in, connected_agent)
             VALUES (?1, ?2, ?3, ?4)",
            (
                self.channel_id.as_str(),
                self.sender_id.as_str(),
                self.is_logged_in as i64,
                connected_agent,
            ),
        )
        .await?;

        Ok(Self {
            channel_id: self.channel_id.clone(),
            sender_id: self.sender_id.clone(),
            is_logged_in: self.is_logged_in,
            connected_agent: self.connected_agent.clone(),
        })
    }
}

impl Entity for SenderGateState {
    fn create_table(conn: &Connection) -> impl Future<Output = anyhow::Result<()>> {
        async move {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS sender_gate_states (
                    channel_id TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    is_logged_in INTEGER NOT NULL DEFAULT 0,
                    connected_agent TEXT,
                    PRIMARY KEY (channel_id, sender_id)
                )",
                (),
            )
            .await?;
            Ok(())
        }
    }
}
