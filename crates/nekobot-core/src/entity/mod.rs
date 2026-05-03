//! Database entity layer backed by libSQL (turso).
//!
//! Each entity type corresponds to a SQL table and implements [`Entity`]
//! for `CREATE TABLE IF NOT EXISTS` semantics.

use turso::Connection;

pub mod channel_chat_agent;
pub mod message;
pub mod persona;
pub mod sender_gate_state;
pub mod session;

pub(crate) async fn enable_foreign_keys(conn: &Connection) -> anyhow::Result<()> {
    conn.execute("PRAGMA foreign_keys = ON", ()).await?;
    Ok(())
}

/// Trait for types that map to a database table.
///
/// Implementations should issue a `CREATE TABLE IF NOT EXISTS` statement
/// that is safe to call multiple times.
pub trait Entity {
    fn create_table(conn: &Connection) -> impl Future<Output = anyhow::Result<()>>;
}

/// Generates a `collect_rows` method for an entity that has a `from_row` method.
///
/// Usage: `collect_rows!(EntityName);`
macro_rules! collect_rows {
    ($entity:ident) => {
        async fn collect_rows(rows: &mut turso::Rows) -> anyhow::Result<Vec<$entity>> {
            let mut items = Vec::new();
            while let Some(row) = rows.next().await? {
                items.push($entity::from_row(&row)?);
            }
            Ok(items)
        }
    };
}

pub(crate) use collect_rows;

/// Shared test helper: creates an in-memory database connection.
#[cfg(test)]
pub(crate) async fn test_connection() -> anyhow::Result<turso::Connection> {
    let db = turso::Builder::new_local(":memory:").build().await?;
    let conn = db.connect()?;
    Ok(conn)
}
