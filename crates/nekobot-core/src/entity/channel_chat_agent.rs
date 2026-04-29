//! Channel-chat-agent mapping entity — links a channel+chat combination to an agent session.

use nekobot_channel::{ChannelId, ChannelName, ChatId, ChatName, ReplyTarget};
use turso::Connection;

use crate::entity::{Entity, enable_foreign_keys, session::Session};

macro_rules! string_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

/// Newtype for the `channel_chat_agents` table primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelChatAgentId(i64);

impl ChannelChatAgentId {
    /// Return the underlying `i64` value.
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl From<i64> for ChannelChatAgentId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

/// Newtype for a session foreign key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(i64);

impl SessionId {
    /// Return the underlying `i64` value.
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl From<i64> for SessionId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

string_newtype!(AgentName);

/// A row in the `channel_chat_agents` table — binds a channel+chat to an agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelChatAgent {
    pub id: ChannelChatAgentId,
    pub channel_id: ChannelId,
    pub channel_name: ChannelName,
    pub chat_id: ChatId,
    pub chat_name: ChatName,
    pub reply_target: ReplyTarget,
    pub agent_name: AgentName,
    pub session_id: SessionId,
}

/// Data needed to insert a new [`ChannelChatAgent`] row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChannelChatAgent {
    pub channel_id: ChannelId,
    pub channel_name: ChannelName,
    pub chat_id: ChatId,
    pub chat_name: ChatName,
    pub reply_target: ReplyTarget,
    pub agent_name: AgentName,
    pub session_id: SessionId,
}

impl ChannelChatAgent {
    /// Insert a new channel-chat-agent mapping and return it.
    pub async fn create(
        conn: &Connection,
        new_mapping: NewChannelChatAgent,
    ) -> anyhow::Result<Self> {
        enable_foreign_keys(conn).await?;

        conn.execute(
            "INSERT INTO channel_chat_agents
                (channel_id, channel_name, chat_id, chat_name, reply_target, agent_name, session_id)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                new_mapping.channel_id.as_str(),
                new_mapping.channel_name.as_str(),
                new_mapping.chat_id.as_str(),
                new_mapping.chat_name.as_str(),
                new_mapping.reply_target.as_str(),
                new_mapping.agent_name.as_str(),
                new_mapping.session_id.as_i64(),
            ),
        )
        .await?;

        Ok(Self {
            id: conn.last_insert_rowid().into(),
            channel_id: new_mapping.channel_id,
            channel_name: new_mapping.channel_name,
            chat_id: new_mapping.chat_id,
            chat_name: new_mapping.chat_name,
            reply_target: new_mapping.reply_target,
            agent_name: new_mapping.agent_name,
            session_id: new_mapping.session_id,
        })
    }

    /// Look up a mapping by its natural key (channel, chat, agent).
    pub async fn get_by_channel_chat_agent(
        conn: &Connection,
        channel_id: &ChannelId,
        chat_id: &ChatId,
        agent_name: &AgentName,
    ) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, channel_id, channel_name, chat_id, chat_name, reply_target, agent_name, session_id
                    FROM channel_chat_agents
                    WHERE channel_id = ?1 AND chat_id = ?2 AND agent_name = ?3",
                (channel_id.as_str(), chat_id.as_str(), agent_name.as_str()),
            )
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    /// Look up a mapping by its session id.
    pub async fn get_by_session_id(
        conn: &Connection,
        session_id: SessionId,
    ) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, channel_id, channel_name, chat_id, chat_name, reply_target, agent_name, session_id
                    FROM channel_chat_agents
                    WHERE session_id = ?1",
                (session_id.as_i64(),),
            )
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    /// Update the cached channel/chat metadata and reply target for a mapping.
    pub async fn update_chat_cache(
        conn: &Connection,
        id: ChannelChatAgentId,
        channel_name: ChannelName,
        chat_name: ChatName,
        reply_target: ReplyTarget,
    ) -> anyhow::Result<Option<Self>> {
        enable_foreign_keys(conn).await?;

        let changed = conn
            .execute(
                "UPDATE channel_chat_agents
                    SET channel_name = ?1, chat_name = ?2, reply_target = ?3
                    WHERE id = ?4",
                (
                    channel_name.as_str(),
                    chat_name.as_str(),
                    reply_target.as_str(),
                    id.as_i64(),
                ),
            )
            .await?;

        if changed == 0 {
            return Ok(None);
        }

        Self::get_by_id(conn, id).await
    }

    /// Look up a mapping by its primary key.
    pub async fn get_by_id(
        conn: &Connection,
        id: ChannelChatAgentId,
    ) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, channel_id, channel_name, chat_id, chat_name, reply_target, agent_name, session_id
                    FROM channel_chat_agents
                    WHERE id = ?1",
                (id.as_i64(),),
            )
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    fn from_row(row: &turso::Row) -> anyhow::Result<Self> {
        let id: i64 = row.get(0)?;
        let channel_id: String = row.get(1)?;
        let channel_name: String = row.get(2)?;
        let chat_id: String = row.get(3)?;
        let chat_name: String = row.get(4)?;
        let reply_target: String = row.get(5)?;
        let agent_name: String = row.get(6)?;
        let session_id: i64 = row.get(7)?;

        Ok(Self {
            id: id.into(),
            channel_id: channel_id.into(),
            channel_name: channel_name.into(),
            chat_id: chat_id.into(),
            chat_name: chat_name.into(),
            reply_target: reply_target.into(),
            agent_name: agent_name.into(),
            session_id: session_id.into(),
        })
    }
}

impl Entity for ChannelChatAgent {
    async fn create_table(conn: &Connection) -> anyhow::Result<()> {
        Session::create_table(conn).await?;
        enable_foreign_keys(conn).await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS channel_chat_agents (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    channel_id TEXT NOT NULL,
                    channel_name TEXT NOT NULL,
                    chat_id TEXT NOT NULL,
                    chat_name TEXT NOT NULL,
                    reply_target TEXT NOT NULL,
                    agent_name TEXT NOT NULL,
                    session_id INTEGER NOT NULL UNIQUE,
                    UNIQUE(channel_id, chat_id, agent_name),
                    FOREIGN KEY(session_id) REFERENCES sessions(id)
                )",
            (),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use turso::Builder;

    use super::*;

    async fn connection() -> anyhow::Result<(Connection, Session)> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        ChannelChatAgent::create_table(&conn).await?;
        let session = Session::create(&conn, "Neko").await?;
        Ok((conn, session))
    }

    #[tokio::test]
    async fn channel_chat_agent_crud() -> anyhow::Result<()> {
        let (conn, session) = connection().await?;
        let mapping = ChannelChatAgent::create(
            &conn,
            NewChannelChatAgent {
                channel_id: ChannelId::from("qq-main"),
                channel_name: ChannelName::from("QQ"),
                chat_id: ChatId::from("chat-1"),
                chat_name: ChatName::from("Alice"),
                reply_target: ReplyTarget::from("target-1"),
                agent_name: AgentName::from("Neko"),
                session_id: SessionId::from(session.id),
            },
        )
        .await?;

        assert_eq!(
            ChannelChatAgent::get_by_channel_chat_agent(
                &conn,
                &ChannelId::from("qq-main"),
                &ChatId::from("chat-1"),
                &AgentName::from("Neko"),
            )
            .await?,
            Some(mapping.clone())
        );

        let updated = ChannelChatAgent::update_chat_cache(
            &conn,
            mapping.id,
            ChannelName::from("QQ Main"),
            ChatName::from("Alice Updated"),
            ReplyTarget::from("target-2"),
        )
        .await?
        .expect("mapping should exist");

        assert_eq!(updated.channel_name, ChannelName::from("QQ Main"));
        assert_eq!(updated.chat_name, ChatName::from("Alice Updated"));
        assert_eq!(updated.reply_target, ReplyTarget::from("target-2"));

        Ok(())
    }
}
