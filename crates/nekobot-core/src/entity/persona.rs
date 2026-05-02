//! Persona entity — persistent agent personality.

use turso::Connection;

/// Create the `personae` table.
pub async fn create_table(conn: &Connection) -> anyhow::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS personae (
            agent_name TEXT NOT NULL PRIMARY KEY,
            persona TEXT NOT NULL
        )",
        (),
    )
    .await?;
    Ok(())
}

/// Get the stored persona for an agent.
pub async fn get(conn: &Connection, agent_name: &str) -> anyhow::Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT persona FROM personae WHERE agent_name = ?1",
            (agent_name,),
        )
        .await?;
    if let Some(row) = rows.next().await? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Insert or replace a persona for an agent.
pub async fn upsert(conn: &Connection, agent_name: &str, persona: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO personae (agent_name, persona) VALUES (?1, ?2)",
        (agent_name, persona),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use turso::Builder;

    async fn conn() -> Connection {
        Builder::new_local(":memory:").build().await.unwrap().connect().unwrap()
    }

    #[tokio::test]
    async fn upsert_get() {
        let c = conn().await;
        create_table(&c).await.unwrap();
        assert!(get(&c, "Neko").await.unwrap().is_none());

        upsert(&c, "Neko", "speak in Chinese").await.unwrap();
        assert_eq!(get(&c, "Neko").await.unwrap().unwrap(), "speak in Chinese");

        upsert(&c, "Neko", "speak in Japanese").await.unwrap();
        assert_eq!(get(&c, "Neko").await.unwrap().unwrap(), "speak in Japanese");
    }
}
