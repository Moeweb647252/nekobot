use turso::Connection;

use crate::entity::{Entity, enable_foreign_keys};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub id: i64,
    pub name: String,
    pub model: String,
}

impl Agent {
    pub async fn create(
        conn: &Connection,
        name: impl Into<String>,
        model: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let name = name.into();
        let model = model.into();

        conn.execute(
            "INSERT INTO agents (name, model) VALUES (?1, ?2)",
            (name.as_str(), model.as_str()),
        )
        .await?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            name,
            model,
        })
    }

    pub async fn get(conn: &Connection, id: i64) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query("SELECT id, name, model FROM agents WHERE id = ?1", (id,))
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    pub async fn list(conn: &Connection) -> anyhow::Result<Vec<Self>> {
        let mut rows = conn
            .query("SELECT id, name, model FROM agents ORDER BY id", ())
            .await?;
        let mut agents = Vec::new();

        while let Some(row) = rows.next().await? {
            agents.push(Self::from_row(&row)?);
        }

        Ok(agents)
    }

    pub async fn update(
        conn: &Connection,
        id: i64,
        name: impl Into<String>,
        model: impl Into<String>,
    ) -> anyhow::Result<Option<Self>> {
        let name = name.into();
        let model = model.into();
        let changed = conn
            .execute(
                "UPDATE agents SET name = ?1, model = ?2 WHERE id = ?3",
                (name.as_str(), model.as_str(), id),
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
            .execute("DELETE FROM agents WHERE id = ?1", (id,))
            .await?;

        Ok(changed > 0)
    }

    fn from_row(row: &turso::Row) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            model: row.get(2)?,
        })
    }
}

impl Entity for Agent {
    async fn create_table(conn: &Connection) -> anyhow::Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS agents (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    model TEXT NOT NULL
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
        Agent::create_table(&conn).await?;
        Ok(conn)
    }

    #[tokio::test]
    async fn agent_crud() -> anyhow::Result<()> {
        let conn = connection().await?;

        let first = Agent::create(&conn, "Neko", "gpt-5.4").await?;
        let second = Agent::create(&conn, "Mimi", "gpt-5.4-mini").await?;

        assert_eq!(first.id, 1);
        assert_eq!(Agent::get(&conn, first.id).await?, Some(first.clone()));
        assert_eq!(
            Agent::list(&conn).await?,
            vec![first.clone(), second.clone()]
        );

        let updated = Agent::update(&conn, first.id, "Neko Prime", "gpt-5.5")
            .await?
            .expect("agent should exist");
        assert_eq!(
            updated,
            Agent {
                id: first.id,
                name: "Neko Prime".to_string(),
                model: "gpt-5.5".to_string(),
            }
        );
        assert_eq!(Agent::update(&conn, 999, "Missing", "gpt-5.4").await?, None);

        assert!(Agent::delete(&conn, second.id).await?);
        assert!(!Agent::delete(&conn, 999).await?);
        assert_eq!(Agent::get(&conn, second.id).await?, None);

        Ok(())
    }
}
