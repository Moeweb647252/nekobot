//! Database entity layer backed by libSQL (turso).
//!
//! Each entity type corresponds to a SQL table and implements [`Entity`]
//! for `CREATE TABLE IF NOT EXISTS` semantics.

use turso::Connection;

pub mod channel_chat_agent;
pub mod message;
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
