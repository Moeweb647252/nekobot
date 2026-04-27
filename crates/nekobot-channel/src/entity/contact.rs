use turso::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub id: i64,
    pub session_id: i64,
    pub name: String,
    pub target: String,
}

impl Contact {
    pub async fn create_table(conn: &Connection) -> anyhow::Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS contacts (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id INTEGER NOT NULL,
                    name TEXT NOT NULL UNIQUE,
                    target TEXT NOT NULL
                )",
            (),
        )
        .await?;

        Ok(())
    }

    pub async fn create(
        conn: &Connection,
        session_id: i64,
        name: impl Into<String>,
        target: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let name = name.into();
        let target = target.into();

        conn.execute(
            "INSERT INTO contacts (session_id, name, target) VALUES (?1, ?2, ?3)",
            (session_id, name.as_str(), target.as_str()),
        )
        .await?;

        Ok(Self {
            id: conn.last_insert_rowid(),
            session_id,
            name,
            target,
        })
    }

    pub async fn get_by_name(
        conn: &Connection,
        name: impl AsRef<str>,
    ) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, session_id, name, target FROM contacts WHERE name = ?1",
                (name.as_ref(),),
            )
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    pub async fn update_target(
        conn: &Connection,
        id: i64,
        target: impl Into<String>,
    ) -> anyhow::Result<Option<Self>> {
        let target = target.into();
        let changed = conn
            .execute(
                "UPDATE contacts SET target = ?1 WHERE id = ?2",
                (target.as_str(), id),
            )
            .await?;

        if changed == 0 {
            return Ok(None);
        }

        Self::get_by_id(conn, id).await
    }

    pub async fn get_by_id(conn: &Connection, id: i64) -> anyhow::Result<Option<Self>> {
        let mut rows = conn
            .query(
                "SELECT id, session_id, name, target FROM contacts WHERE id = ?1",
                (id,),
            )
            .await?;

        rows.next()
            .await?
            .map(|row| Self::from_row(&row))
            .transpose()
    }

    fn from_row(row: &turso::Row) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            session_id: row.get(1)?,
            name: row.get(2)?,
            target: row.get(3)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use turso::Builder;

    use super::*;

    async fn connection() -> anyhow::Result<Connection> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        Contact::create_table(&conn).await?;
        Ok(conn)
    }

    #[tokio::test]
    async fn contact_crud() -> anyhow::Result<()> {
        let conn = connection().await?;
        let contact = Contact::create(&conn, 42, "Alice", "target-1").await?;

        assert_eq!(
            Contact::get_by_name(&conn, "Alice").await?,
            Some(contact.clone())
        );

        let updated = Contact::update_target(&conn, contact.id, "target-2")
            .await?
            .expect("contact should exist");
        assert_eq!(updated.target, "target-2");
        assert_eq!(updated.session_id, 42);

        Ok(())
    }
}
