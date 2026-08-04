use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("database schema requires ownership recovery")]
    OwnershipRecoveryRequired { table: String },
    #[error("database schema version is newer than this application supports")]
    UnsupportedSchemaVersion { found: i64 },
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
}

pub fn migrate(connection: &mut Connection) -> Result<(), MigrationError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(MigrationError::UnsupportedSchemaVersion { found: version });
    }
    for table in ["sessions", "session_briefs", "timeline_events", "settings"] {
        let exists: Option<String> = transaction
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            let owned = transaction
                .prepare(&format!("PRAGMA table_info({table})"))?
                .query_map([], |row| row.get::<_, String>(1))?
                .any(|column| column.ok().as_deref() == Some("workspace_id"));
            if !owned {
                return Err(MigrationError::OwnershipRecoveryRequired {
                    table: table.to_owned(),
                });
            }
        }
    }
    if version == 0 {
        transaction.execute_batch(
            "CREATE TABLE sessions (
                workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, title TEXT, language TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL, completed_at_ms INTEGER, PRIMARY KEY (workspace_id, session_id)
            );
            CREATE TABLE session_briefs (
                workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, summary TEXT NOT NULL, updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (workspace_id, session_id),
                FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
            );
            CREATE TABLE timeline_events (
                event_id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL, session_id TEXT NOT NULL,
                host_sequence INTEGER NOT NULL, source_generation INTEGER NOT NULL, source_sequence INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL, kind TEXT NOT NULL, correlation_id TEXT, request_id TEXT, turn_id TEXT,
                payload_json TEXT NOT NULL,
                UNIQUE (workspace_id, session_id, host_sequence),
                FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
            );
            CREATE TABLE settings (
                workspace_id TEXT NOT NULL, setting_key TEXT NOT NULL, value_json TEXT NOT NULL,
                PRIMARY KEY (workspace_id, setting_key)
            );"
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{migrate, MigrationError, SCHEMA_VERSION};
    use rusqlite::Connection;
    #[test]
    fn empty_database_migrates_to_current_owned_schema() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        for table in ["sessions", "session_briefs", "timeline_events", "settings"] {
            let has_workspace = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .any(|column| column.unwrap() == "workspace_id");
            assert!(has_workspace, "{table} must carry workspace ownership");
        }
    }
    #[test]
    fn unowned_legacy_schema_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE sessions (session_id TEXT PRIMARY KEY);")
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(MigrationError::OwnershipRecoveryRequired { .. })
        ));
    }
    #[test]
    fn newer_schema_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(MigrationError::UnsupportedSchemaVersion { .. })
        ));
    }
}
