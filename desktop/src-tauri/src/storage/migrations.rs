use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("database schema requires ownership recovery")]
    OwnershipRecoveryRequired { table: String },
    #[error("database schema version is newer than this application supports")]
    UnsupportedSchemaVersion { found: i64 },
    #[error("database schema integrity check failed: {reason}")]
    SchemaIntegrity { reason: String },
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
    validate_current_schema(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn validate_current_schema(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    validate_table(
        transaction,
        "sessions",
        &[
            "workspace_id",
            "session_id",
            "title",
            "language",
            "started_at_ms",
            "completed_at_ms",
        ],
        &["workspace_id", "session_id"],
    )?;
    validate_table(
        transaction,
        "session_briefs",
        &["workspace_id", "session_id", "summary", "updated_at_ms"],
        &["workspace_id", "session_id"],
    )?;
    validate_table(
        transaction,
        "timeline_events",
        &[
            "event_id",
            "workspace_id",
            "session_id",
            "host_sequence",
            "source_generation",
            "source_sequence",
            "timestamp_ms",
            "kind",
            "correlation_id",
            "request_id",
            "turn_id",
            "payload_json",
        ],
        &["event_id"],
    )?;
    validate_table(
        transaction,
        "settings",
        &["workspace_id", "setting_key", "value_json"],
        &["workspace_id", "setting_key"],
    )?;

    for table in ["session_briefs", "timeline_events"] {
        if !has_workspace_session_cascade(transaction, table)? {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!(
                    "{table} must reference sessions(workspace_id, session_id) with ON DELETE CASCADE"
                ),
            });
        }
    }
    if !has_unique_index(
        transaction,
        "timeline_events",
        &["workspace_id", "session_id", "host_sequence"],
    )? {
        return Err(MigrationError::SchemaIntegrity {
            reason: "timeline_events must uniquely order events within a workspace session"
                .to_owned(),
        });
    }
    Ok(())
}

fn validate_table(
    transaction: &Transaction<'_>,
    table: &str,
    required_columns: &[&str],
    expected_primary_key: &[&str],
) -> Result<(), MigrationError> {
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(MigrationError::SchemaIntegrity {
            reason: format!("required table {table} is missing"),
        });
    }

    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for required in required_columns {
        if !columns.iter().any(|(column, _)| column == required) {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!("required column {table}.{required} is missing"),
            });
        }
    }
    let mut primary_key = columns
        .iter()
        .filter(|(_, position)| *position > 0)
        .map(|(column, position)| (*position, column.as_str()))
        .collect::<Vec<_>>();
    primary_key.sort_by_key(|(position, _)| *position);
    let primary_key = primary_key
        .into_iter()
        .map(|(_, column)| column)
        .collect::<Vec<_>>();
    if primary_key != expected_primary_key {
        return Err(MigrationError::SchemaIntegrity {
            reason: format!("{table} has an unexpected primary key"),
        });
    }
    Ok(())
}

fn has_workspace_session_cascade(
    transaction: &Transaction<'_>,
    table: &str,
) -> Result<bool, MigrationError> {
    let mut statement = transaction.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
    let foreign_keys = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(foreign_keys.iter().any(|first| {
        first.1 == 0
            && first.2 == "sessions"
            && first.3 == "workspace_id"
            && first.4 == "workspace_id"
            && first.5.eq_ignore_ascii_case("CASCADE")
            && foreign_keys.iter().any(|second| {
                second.0 == first.0
                    && second.1 == 1
                    && second.2 == "sessions"
                    && second.3 == "session_id"
                    && second.4 == "session_id"
                    && second.5.eq_ignore_ascii_case("CASCADE")
            })
    }))
}

fn has_unique_index(
    transaction: &Transaction<'_>,
    table: &str,
    expected_columns: &[&str],
) -> Result<bool, MigrationError> {
    let mut statement = transaction.prepare(&format!("PRAGMA index_list({table})"))?;
    let indexes = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, bool>(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (index, unique) in indexes {
        if !unique {
            continue;
        }
        let quoted = index.replace('"', "\"\"");
        let mut index_statement =
            transaction.prepare(&format!("PRAGMA index_info(\"{quoted}\")"))?;
        let columns = index_statement
            .query_map([], |row| row.get::<_, String>(2))?
            .collect::<Result<Vec<_>, _>>()?;
        if columns.iter().map(String::as_str).collect::<Vec<_>>() == expected_columns {
            return Ok(true);
        }
    }
    Ok(false)
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

    #[test]
    fn damaged_current_schema_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection.execute_batch("DROP TABLE settings;").unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn current_schema_with_damaged_relationship_topology_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE timeline_events;
                CREATE TABLE timeline_events (
                    event_id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL,
                    session_id TEXT NOT NULL, host_sequence INTEGER NOT NULL,
                    source_generation INTEGER NOT NULL, source_sequence INTEGER NOT NULL,
                    timestamp_ms INTEGER NOT NULL, kind TEXT NOT NULL, correlation_id TEXT,
                    request_id TEXT, turn_id TEXT, payload_json TEXT NOT NULL
                );",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }
}
