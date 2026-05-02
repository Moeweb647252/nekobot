//! Channel credential entity — persistent credentials for channels that need login.

use turso::Connection;

/// Create the `channel_credentials` table.
pub async fn create_table(conn: &Connection) -> anyhow::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS channel_credentials (
            channel_name TEXT NOT NULL PRIMARY KEY,
            credentials TEXT NOT NULL
        )",
        (),
    )
    .await?;
    Ok(())
}

/// Get stored credentials for a channel by its user-defined name.
pub async fn get(conn: &Connection, channel_name: &str) -> anyhow::Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT credentials FROM channel_credentials WHERE channel_name = ?1",
            (channel_name,),
        )
        .await?;
    if let Some(row) = rows.next().await? {
        let creds: String = row.get(0)?;
        Ok(Some(creds))
    } else {
        Ok(None)
    }
}

/// Insert or replace credentials for a channel.
pub async fn upsert(
    conn: &Connection,
    channel_name: &str,
    credentials: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO channel_credentials (channel_name, credentials) VALUES (?1, ?2)",
        (channel_name, credentials),
    )
    .await?;
    Ok(())
}

/// Delete credentials for a channel.
pub async fn delete(conn: &Connection, channel_name: &str) -> anyhow::Result<bool> {
    let changed = conn
        .execute(
            "DELETE FROM channel_credentials WHERE channel_name = ?1",
            (channel_name,),
        )
        .await?;
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use turso::Builder;

    async fn conn() -> Connection {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        db.connect().unwrap()
    }

    #[tokio::test]
    async fn upsert_get_delete() {
        let c = conn().await;
        create_table(&c).await.unwrap();

        // Get nonexistent
        assert!(get(&c, "test").await.unwrap().is_none());

        // Upsert
        upsert(&c, "test", r#"{"token":"abc"}"#).await.unwrap();
        assert_eq!(get(&c, "test").await.unwrap().unwrap(), r#"{"token":"abc"}"#);

        // Upsert replaces
        upsert(&c, "test", r#"{"token":"xyz"}"#).await.unwrap();
        assert_eq!(get(&c, "test").await.unwrap().unwrap(), r#"{"token":"xyz"}"#);

        // Delete
        assert!(delete(&c, "test").await.unwrap());
        assert!(!delete(&c, "test").await.unwrap());
        assert!(get(&c, "test").await.unwrap().is_none());
    }
}
