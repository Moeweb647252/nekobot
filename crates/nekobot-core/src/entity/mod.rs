use turso::Connection;

pub mod message;
pub mod session;

pub(crate) async fn enable_foreign_keys(conn: &Connection) -> anyhow::Result<()> {
    conn.execute("PRAGMA foreign_keys = ON", ()).await?;
    Ok(())
}

pub trait Entity {
    fn create_table(conn: &Connection) -> impl Future<Output = anyhow::Result<()>>;
}
