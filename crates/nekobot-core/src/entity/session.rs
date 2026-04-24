use turso::Connection;

use crate::entity::{Entity, agent::Agent, enable_foreign_keys};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: i64,
    pub agent_id: i64,
}

impl Session {
    pub async fn create(conn: &Connection, agent_id: i64) -> anyhow::Result<Self> {
        enable_foreign_keys(conn).await?;
        conn.execute("INSERT INTO sessions (agent_id) VALUES (?1)", (agent_id,))
            .await?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            agent_id,
        })
    }

    pub async fn get(conn: &Connection, id: i64) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query("SELECT id, agent_id FROM sessions WHERE id = ?1", (id,))
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    pub async fn list(conn: &Connection) -> anyhow::Result<Vec<Self>> {
        let mut rows = conn
            .query("SELECT id, agent_id FROM sessions ORDER BY id", ())
            .await?;
        Self::collect_rows(&mut rows).await
    }

    pub async fn list_by_agent(conn: &Connection, agent_id: i64) -> anyhow::Result<Vec<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, agent_id FROM sessions WHERE agent_id = ?1 ORDER BY id",
                (agent_id,),
            )
            .await?;
        Self::collect_rows(&mut rows).await
    }

    pub async fn update(conn: &Connection, id: i64, agent_id: i64) -> anyhow::Result<Option<Self>> {
        enable_foreign_keys(conn).await?;
        let changed = conn
            .execute(
                "UPDATE sessions SET agent_id = ?1 WHERE id = ?2",
                (agent_id, id),
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
            agent_id: row.get(1)?,
        })
    }
}

impl Entity for Session {
    async fn create_table(conn: &Connection) -> anyhow::Result<()> {
        Agent::create_table(conn).await?;
        enable_foreign_keys(conn).await?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    agent_id INTEGER NOT NULL,
                    FOREIGN KEY(agent_id) REFERENCES agents(id)
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
        let first_agent = Agent::create(&conn, "Neko", "gpt-5.4").await?;
        let second_agent = Agent::create(&conn, "Mimi", "gpt-5.4-mini").await?;

        let first = Session::create(&conn, first_agent.id).await?;
        let second = Session::create(&conn, first_agent.id).await?;
        let third = Session::create(&conn, second_agent.id).await?;

        assert_eq!(first.id, 1);
        assert_eq!(Session::get(&conn, first.id).await?, Some(first.clone()));
        assert_eq!(
            Session::list(&conn).await?,
            vec![first.clone(), second.clone(), third.clone()]
        );
        assert_eq!(
            Session::list_by_agent(&conn, first_agent.id).await?,
            vec![first.clone(), second.clone()]
        );

        let updated = Session::update(&conn, first.id, second_agent.id)
            .await?
            .expect("session should exist");
        assert_eq!(
            updated,
            Session {
                id: first.id,
                agent_id: second_agent.id,
            }
        );
        assert_eq!(Session::update(&conn, 999, first_agent.id).await?, None);

        assert!(Session::delete(&conn, second.id).await?);
        assert!(!Session::delete(&conn, 999).await?);
        assert_eq!(Session::get(&conn, second.id).await?, None);

        Ok(())
    }

    #[tokio::test]
    async fn session_foreign_key_is_enforced() -> anyhow::Result<()> {
        let conn = connection().await?;
        let agent = Agent::create(&conn, "Neko", "gpt-5.4").await?;

        assert!(Session::create(&conn, 999).await.is_err());

        let session = Session::create(&conn, agent.id).await?;
        assert!(Agent::delete(&conn, agent.id).await.is_err());

        assert!(Session::delete(&conn, session.id).await?);
        assert!(Agent::delete(&conn, agent.id).await?);

        Ok(())
    }
}
