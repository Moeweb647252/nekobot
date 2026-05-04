//! Memory entity — vector-backed persistent memory storage.

use turso::Connection;

/// A single memory entry with optional vector embedding.
#[derive(Debug, Clone)]
pub(crate) struct MemoryRow {
    pub id: i64,
    pub content: String,
}

/// Create the `memories` table if it doesn't exist.
pub async fn create_table(conn: &Connection) -> anyhow::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_name TEXT NOT NULL,
            content TEXT NOT NULL,
            embedding BLOB,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        (),
    )
    .await?;
    Ok(())
}

/// Insert a memory with its embedding.
pub async fn insert(
    conn: &Connection,
    agent_name: &str,
    content: &str,
    embedding: &[f32],
) -> anyhow::Result<i64> {
    let blob = encode_vector(embedding);
    conn.execute(
        "INSERT INTO memories (agent_name, content, embedding) VALUES (?1, ?2, ?3)",
        (agent_name, content, blob),
    )
    .await?;
    Ok(conn.last_insert_rowid())
}

/// Vector similarity search. Returns `(content, distance)` ordered by closest match.
pub async fn search(
    conn: &Connection,
    agent_name: &str,
    query_embedding: &[f32],
    limit: usize,
) -> anyhow::Result<Vec<MemoryRow>> {
    let blob = encode_vector(query_embedding);
    let sql = format!(
        "SELECT id, content, vector_distance_cos(embedding, vector32(?1)) AS dist \
         FROM memories WHERE agent_name = ?2 ORDER BY dist LIMIT {limit}"
    );
    let mut rows = conn.query(&sql, (blob, agent_name)).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        results.push(MemoryRow { id, content });
    }
    Ok(results)
}

/// Delete a memory by id.
pub async fn delete(conn: &Connection, id: i64) -> anyhow::Result<bool> {
    let changed = conn
        .execute("DELETE FROM memories WHERE id = ?1", (id,))
        .await?;
    Ok(changed > 0)
}

/// Encode `&[f32]` as a little-endian byte blob for `vector32()`.
fn encode_vector(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for &f in vec {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use turso::Builder;

    async fn test_conn() -> Connection {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        db.connect().unwrap()
    }

    #[test]
    fn encode_decode_roundtrip() {
        let original = vec![1.0f32, -0.5, 0.25];
        let blob = encode_vector(&original);
        let decoded: Vec<f32> = blob
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        assert_eq!(original, decoded);
    }

    #[tokio::test]
    async fn search_returns_similar() {
        let conn = test_conn().await;
        create_table(&conn).await.unwrap();

        insert(&conn, "test", "cats are great", &[1.0, 0.0])
            .await
            .unwrap();
        insert(&conn, "test", "dogs are loyal", &[0.0, 1.0])
            .await
            .unwrap();

        let results = search(&conn, "test", &[0.9, 0.1], 2).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "cats are great");
    }

    #[tokio::test]
    async fn search_skips_other_agents() {
        let conn = test_conn().await;
        create_table(&conn).await.unwrap();
        insert(&conn, "alice", "alice memory", &[1.0])
            .await
            .unwrap();
        insert(&conn, "bob", "bob memory", &[1.0]).await.unwrap();

        let results = search(&conn, "alice", &[1.0], 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "alice memory");
    }

    #[tokio::test]
    async fn delete_removes_memory() {
        let conn = test_conn().await;
        create_table(&conn).await.unwrap();
        let id = insert(&conn, "test", "temp", &[0.0]).await.unwrap();
        assert!(delete(&conn, id).await.unwrap());
        assert!(!delete(&conn, 999).await.unwrap());
    }
}
