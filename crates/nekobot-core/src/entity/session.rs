use turso::Connection;

use crate::entity::{Entity, enable_foreign_keys};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: i64,
    pub agent_name: String,
}

impl Session {
    pub async fn create(conn: &Connection, agent_name: impl Into<String>) -> anyhow::Result<Self> {
        enable_foreign_keys(conn).await?;

        let agent_name = agent_name.into();
        conn.execute(
            "INSERT INTO sessions (agent_name) VALUES (?1)",
            (agent_name.as_str(),),
        )
        .await?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            agent_name,
        })
    }

    pub async fn get(conn: &Connection, id: i64) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query("SELECT id, agent_name FROM sessions WHERE id = ?1", (id,))
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    pub async fn list(conn: &Connection) -> anyhow::Result<Vec<Self>> {
        let mut rows = conn
            .query("SELECT id, agent_name FROM sessions ORDER BY id", ())
            .await?;
        Self::collect_rows(&mut rows).await
    }

    pub async fn list_by_agent(
        conn: &Connection,
        agent_name: impl AsRef<str>,
    ) -> anyhow::Result<Vec<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, agent_name FROM sessions WHERE agent_name = ?1 ORDER BY id",
                (agent_name.as_ref(),),
            )
            .await?;
        Self::collect_rows(&mut rows).await
    }

    pub async fn update(
        conn: &Connection,
        id: i64,
        agent_name: impl Into<String>,
    ) -> anyhow::Result<Option<Self>> {
        enable_foreign_keys(conn).await?;

        let agent_name = agent_name.into();
        let changed = conn
            .execute(
                "UPDATE sessions SET agent_name = ?1 WHERE id = ?2",
                (agent_name.as_str(), id),
            )
            .await?;

        if changed == 0 {
            return Ok(None);
        }

        Self::get(conn, id).await
    }

    pub async fn delete(conn: &Connection, id: i64) -> anyhow::Result<bool> {
        enable_foreign_keys(conn).await?;
        let changed = conn
            .execute("DELETE FROM sessions WHERE id = ?1", (id,))
            .await?;

        Ok(changed > 0)
    }

    async fn collect_rows(rows: &mut turso::Rows) -> anyhow::Result<Vec<Self>> {
        let mut sessions = Vec::new();

        while let Some(row) = rows.next().await? {
            sessions.push(Self::from_row(&row)?);
        }

        Ok(sessions)
    }

    fn from_row(row: &turso::Row) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            agent_name: row.get(1)?,
        })
    }
}

impl Entity for Session {
    async fn create_table(conn: &Connection) -> anyhow::Result<()> {
        enable_foreign_keys(conn).await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    agent_name TEXT NOT NULL
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

    async fn connection() -> anyhow::Result<Connection> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        Session::create_table(&conn).await?;
        Ok(conn)
    }

    #[tokio::test]
    async fn session_crud() -> anyhow::Result<()> {
        let conn = connection().await?;

        let first = Session::create(&conn, "Neko").await?;
        let second = Session::create(&conn, "Neko").await?;
        let third = Session::create(&conn, "Mimi").await?;

        assert_eq!(first.id, 1);
        assert_eq!(Session::get(&conn, first.id).await?, Some(first.clone()));
        assert_eq!(
            Session::list(&conn).await?,
            vec![first.clone(), second.clone(), third.clone()]
        );
        assert_eq!(
            Session::list_by_agent(&conn, "Neko").await?,
            vec![first.clone(), second.clone()]
        );

        let updated = Session::update(&conn, first.id, "Mimi")
            .await?
            .expect("session should exist");
        assert_eq!(
            updated,
            Session {
                id: first.id,
                agent_name: "Mimi".to_owned(),
            }
        );
        assert_eq!(Session::update(&conn, 999, "Neko").await?, None);

        assert!(Session::delete(&conn, second.id).await?);
        assert!(!Session::delete(&conn, 999).await?);
        assert_eq!(Session::get(&conn, second.id).await?, None);

        Ok(())
    }

    #[tokio::test]
    async fn session_allows_any_agent_name() -> anyhow::Result<()> {
        let conn = connection().await?;

        let session = Session::create(&conn, "configured-agent").await?;

        assert_eq!(session.agent_name, "configured-agent");
        assert_eq!(Session::get(&conn, session.id).await?, Some(session));
        Ok(())
    }
}
