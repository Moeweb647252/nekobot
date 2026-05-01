//! Message entity — persistent chat message storage.

use std::fmt::Display;

use turso::Connection;

use crate::entity::{Entity, collect_rows, enable_foreign_keys, session::Session};

/// A single chat message belonging to a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub content: String,
    pub reasoning_content: Option<String>,
    pub role: String,
    pub session_id: i64,
    /// Tool call ID, used when role is "tool".
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Insert a new message and return it.
    pub async fn create(
        conn: &Connection,
        id: impl Into<String>,
        session_id: i64,
        role: impl Into<String>,
        content: impl Into<String>,
        reasoning_content: Option<String>,
        tool_call_id: Option<String>,
    ) -> anyhow::Result<Self> {
        enable_foreign_keys(conn).await?;

        let id = id.into();
        let role = role.into();
        let content = content.into();

        conn.execute(
            "INSERT INTO messages (id, session_id, role, content, reasoning_content, tool_call_id)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                id.as_str(),
                session_id,
                role.as_str(),
                content.as_str(),
                reasoning_content.as_deref(),
                tool_call_id.as_deref(),
            ),
        )
        .await?;

        Ok(Self {
            id,
            content,
            reasoning_content,
            role,
            session_id,
            tool_call_id,
        })
    }

    /// Look up a message by its text primary key.
    pub async fn get(conn: &Connection, id: impl AsRef<str>) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, content, reasoning_content, role, session_id, tool_call_id
                    FROM messages WHERE id = ?1",
                (id.as_ref(),),
            )
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    /// Return all messages ordered by insertion order.
    pub async fn list(conn: &Connection) -> anyhow::Result<Vec<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, content, reasoning_content, role, session_id, tool_call_id
                    FROM messages ORDER BY rowid",
                (),
            )
            .await?;
        Self::collect_rows(&mut rows).await
    }

    /// Return all messages belonging to a given session, ordered by insertion.
    pub async fn list_by_session(conn: &Connection, session_id: i64) -> anyhow::Result<Vec<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, content, reasoning_content, role, session_id, tool_call_id
                    FROM messages WHERE session_id = ?1 ORDER BY rowid",
                (session_id,),
            )
            .await?;
        Self::collect_rows(&mut rows).await
    }

    /// Update all fields of a message and return the updated row.
    pub async fn update(
        conn: &Connection,
        id: impl AsRef<str>,
        session_id: i64,
        role: impl Into<String>,
        content: impl Into<String>,
        reasoning_content: Option<String>,
    ) -> anyhow::Result<Option<Self>> {
        enable_foreign_keys(conn).await?;

        let id = id.as_ref();
        let role = role.into();
        let content = content.into();
        let changed = conn
            .execute(
                "UPDATE messages
                    SET session_id = ?1, role = ?2, content = ?3, reasoning_content = ?4
                    WHERE id = ?5",
                (
                    session_id,
                    role.as_str(),
                    content.as_str(),
                    reasoning_content.as_deref(),
                    id,
                ),
            )
            .await?;

        if changed == 0 {
            return Ok(None);
        }

        Self::get(conn, id).await
    }

    /// Delete a message by id; returns true if a row was removed.
    pub async fn delete(conn: &Connection, id: impl AsRef<str>) -> anyhow::Result<bool> {
        enable_foreign_keys(conn).await?;
        let changed = conn
            .execute("DELETE FROM messages WHERE id = ?1", (id.as_ref(),))
            .await?;

        Ok(changed > 0)
    }

    collect_rows!(Message);

    fn from_row(row: &turso::Row) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            content: row.get(1)?,
            reasoning_content: row.get(2)?,
            role: row.get(3)?,
            session_id: row.get(4)?,
            tool_call_id: row.get(5)?,
        })
    }
}

impl Entity for Message {
    async fn create_table(conn: &Connection) -> anyhow::Result<()> {
        Session::create_table(conn).await?;
        enable_foreign_keys(conn).await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                    id TEXT PRIMARY KEY,
                    session_id INTEGER NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    reasoning_content TEXT,
                    tool_call_id TEXT,
                    FOREIGN KEY(session_id) REFERENCES sessions(id)
                )",
            (),
        )
        .await?;
        Ok(())
    }
}

/// The role of a message sender (user, assistant, or a custom role).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    Custom(String),
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
            Role::Custom(role) => write!(f, "{role}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use turso::Builder;

    use super::*;

    async fn connection() -> anyhow::Result<Connection> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        Message::create_table(&conn).await?;
        Ok(conn)
    }

    async fn session(conn: &Connection) -> anyhow::Result<Session> {
        Session::create(conn, "Neko").await
    }

    #[tokio::test]
    async fn message_crud() -> anyhow::Result<()> {
        let conn = connection().await?;
        let first_session = session(&conn).await?;
        let second_session = session(&conn).await?;

        let first = Message::create(
            &conn,
            "msg-1",
            first_session.id,
            Role::User.to_string(),
            "hello",
            None,
            None,
        )
        .await?;
        let second = Message::create(
            &conn,
            "msg-2",
            first_session.id,
            Role::Assistant.to_string(),
            "hi",
            Some("short reasoning".to_string()),
            None,
        )
        .await?;
        let third = Message::create(
            &conn,
            "msg-3",
            second_session.id,
            Role::Custom("tool".to_string()).to_string(),
            "tool output",
            None,
            Some("call_123".to_string()),
        )
        .await?;

        assert_eq!(Message::get(&conn, "msg-1").await?, Some(first.clone()));
        assert_eq!(
            Message::list(&conn).await?,
            vec![first.clone(), second.clone(), third.clone()]
        );
        assert_eq!(
            Message::list_by_session(&conn, first_session.id).await?,
            vec![first.clone(), second.clone()]
        );

        let updated = Message::update(
            &conn,
            "msg-1",
            second_session.id,
            Role::Assistant.to_string(),
            "updated",
            Some("updated reasoning".to_string()),
        )
        .await?
        .expect("message should exist");
        assert_eq!(
            updated,
            Message {
                id: "msg-1".to_string(),
                content: "updated".to_string(),
                reasoning_content: Some("updated reasoning".to_string()),
                role: "assistant".to_string(),
                session_id: second_session.id,
                tool_call_id: None,
            }
        );
        assert_eq!(
            Message::update(
                &conn,
                "missing",
                second_session.id,
                Role::User.to_string(),
                "missing",
                None,
            )
            .await?,
            None
        );

        assert!(Message::delete(&conn, "msg-2").await?);
        assert!(!Message::delete(&conn, "missing").await?);
        assert_eq!(Message::get(&conn, "msg-2").await?, None);

        Ok(())
    }

    #[tokio::test]
    async fn message_foreign_key_is_enforced() -> anyhow::Result<()> {
        let conn = connection().await?;
        let session = session(&conn).await?;

        assert!(
            Message::create(
                &conn,
                "orphan",
                999,
                Role::User.to_string(),
                "missing session",
                None,
                None,
            )
            .await
            .is_err()
        );

        let message = Message::create(
            &conn,
            "msg-1",
            session.id,
            Role::User.to_string(),
            "hello",
            None,
            None,
        )
        .await?;

        assert!(Session::delete(&conn, session.id).await.is_err());
        assert!(
            Message::update(
                &conn,
                "msg-1",
                999,
                Role::Assistant.to_string(),
                "invalid session",
                None,
            )
            .await
            .is_err()
        );

        assert!(Message::delete(&conn, message.id).await?);
        assert!(Session::delete(&conn, session.id).await?);

        Ok(())
    }
}
