use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::models::{NewSession, NewSessionBrief, NewTimelineEvent, TimelineEventKind};

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("database schema requires ownership recovery")]
    OwnershipRecoveryRequired { table: String },
    #[error("database schema version is not supported by this application")]
    UnsupportedSchemaVersion { found: i64 },
    #[error("database schema integrity check failed: {reason}")]
    SchemaIntegrity { reason: String },
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
}

pub fn migrate(connection: &mut Connection) -> Result<(), MigrationError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 0 && version != SCHEMA_VERSION {
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
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("session_id", "TEXT", true, 2).with_binary_collation(),
            ColumnSpec::new("title", "TEXT", false, 0),
            ColumnSpec::new("language", "TEXT", true, 0),
            ColumnSpec::new("started_at_ms", "INTEGER", true, 0),
            ColumnSpec::new("completed_at_ms", "INTEGER", false, 0),
        ],
    )?;
    validate_table(
        transaction,
        "session_briefs",
        &[
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("session_id", "TEXT", true, 2).with_binary_collation(),
            ColumnSpec::new("summary", "TEXT", true, 0),
            ColumnSpec::new("updated_at_ms", "INTEGER", true, 0),
        ],
    )?;
    validate_table(
        transaction,
        "timeline_events",
        &[
            ColumnSpec::new("event_id", "TEXT", true, 1),
            ColumnSpec::new("workspace_id", "TEXT", true, 0).with_binary_collation(),
            ColumnSpec::new("session_id", "TEXT", true, 0).with_binary_collation(),
            ColumnSpec::new("host_sequence", "INTEGER", true, 0),
            ColumnSpec::new("source_generation", "INTEGER", true, 0),
            ColumnSpec::new("source_sequence", "INTEGER", true, 0),
            ColumnSpec::new("timestamp_ms", "INTEGER", true, 0),
            ColumnSpec::new("kind", "TEXT", true, 0),
            ColumnSpec::new("correlation_id", "TEXT", false, 0),
            ColumnSpec::new("request_id", "TEXT", false, 0),
            ColumnSpec::new("turn_id", "TEXT", false, 0),
            ColumnSpec::new("payload_json", "TEXT", true, 0),
        ],
    )?;
    validate_table(
        transaction,
        "settings",
        &[
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("setting_key", "TEXT", true, 2),
            ColumnSpec::new("value_json", "TEXT", true, 0),
        ],
    )?;

    validate_unique_indexes(
        transaction,
        "sessions",
        &[UniqueIndexSpec::new("pk", &["workspace_id", "session_id"])],
    )?;
    validate_unique_indexes(
        transaction,
        "session_briefs",
        &[UniqueIndexSpec::new("pk", &["workspace_id", "session_id"])],
    )?;
    validate_unique_indexes(
        transaction,
        "timeline_events",
        &[
            UniqueIndexSpec::new("pk", &["event_id"]),
            UniqueIndexSpec::new("u", &["workspace_id", "session_id", "host_sequence"]),
        ],
    )?;
    validate_unique_indexes(
        transaction,
        "settings",
        &[UniqueIndexSpec::new("pk", &["workspace_id", "setting_key"])],
    )?;

    for table in ["sessions", "settings"] {
        if !foreign_keys(transaction, table)?.is_empty() {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!("{table} has unexpected foreign keys"),
            });
        }
    }
    for table in ["session_briefs", "timeline_events"] {
        if !has_workspace_session_cascade(transaction, table)? {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!(
                    "{table} must reference sessions(workspace_id, session_id) with ON DELETE CASCADE"
                ),
            });
        }
    }
    let mut foreign_key_check = transaction.prepare("PRAGMA foreign_key_check")?;
    if foreign_key_check.query([])?.next()?.is_some() {
        return Err(MigrationError::SchemaIntegrity {
            reason: "the database contains rows that violate ownership relationships".to_owned(),
        });
    }
    validate_current_rows(transaction)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct ColumnSpec {
    name: &'static str,
    affinity: &'static str,
    not_null: bool,
    primary_key_position: i64,
    binary_collation: bool,
}

impl ColumnSpec {
    const fn new(
        name: &'static str,
        affinity: &'static str,
        not_null: bool,
        primary_key_position: i64,
    ) -> Self {
        Self {
            name,
            affinity,
            not_null,
            primary_key_position,
            binary_collation: false,
        }
    }

    const fn with_binary_collation(mut self) -> Self {
        self.binary_collation = true;
        self
    }
}

struct TableColumn {
    name: String,
    affinity: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
    hidden: i64,
}

fn validate_table(
    transaction: &Transaction<'_>,
    table: &str,
    required_columns: &[ColumnSpec],
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

    let mut statement = transaction.prepare(&format!("PRAGMA table_xinfo({table})"))?;
    let columns = statement
        .query_map([], |row| {
            Ok(TableColumn {
                name: row.get(1)?,
                affinity: row.get(2)?,
                not_null: row.get(3)?,
                default_value: row.get(4)?,
                primary_key_position: row.get(5)?,
                hidden: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for required in required_columns {
        let Some(column) = columns.iter().find(|column| column.name == required.name) else {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!("required column {table}.{} is missing", required.name),
            });
        };
        if !column.affinity.eq_ignore_ascii_case(required.affinity)
            || column.not_null != required.not_null
            || column.primary_key_position != required.primary_key_position
            || column.hidden != 0
        {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!(
                    "column {table}.{} has an incompatible definition",
                    required.name
                ),
            });
        }
        if required.binary_collation {
            let (_, collation, _, _, _) =
                transaction.column_metadata(None::<&str>, table, required.name)?;
            if !collation.is_some_and(|value| value.to_bytes().eq_ignore_ascii_case(b"BINARY")) {
                return Err(MigrationError::SchemaIntegrity {
                    reason: format!(
                        "ownership column {table}.{} must use BINARY collation",
                        required.name
                    ),
                });
            }
        }
    }
    for column in &columns {
        let required = required_columns
            .iter()
            .any(|required| required.name == column.name);
        if !required && !extra_column_is_insert_compatible(column) {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!(
                    "extra column {table}.{} cannot be populated by current writes",
                    column.name
                ),
            });
        }
    }
    let mut primary_key = required_columns
        .iter()
        .filter(|column| column.primary_key_position > 0)
        .map(|column| (column.primary_key_position, column.name))
        .collect::<Vec<_>>();
    primary_key.sort_by_key(|(position, _)| *position);
    let primary_key = primary_key
        .into_iter()
        .map(|(_, column)| column)
        .collect::<Vec<_>>();
    let mut actual_primary_key = columns
        .iter()
        .filter(|column| column.primary_key_position > 0)
        .map(|column| (column.primary_key_position, column.name.as_str()))
        .collect::<Vec<_>>();
    actual_primary_key.sort_by_key(|(position, _)| *position);
    let actual_primary_key = actual_primary_key
        .into_iter()
        .map(|(_, column)| column)
        .collect::<Vec<_>>();
    if actual_primary_key != primary_key {
        return Err(MigrationError::SchemaIntegrity {
            reason: format!("{table} has an unexpected primary key"),
        });
    }
    Ok(())
}

fn extra_column_is_insert_compatible(column: &TableColumn) -> bool {
    if !column.not_null {
        return true;
    }
    column.hidden == 0
        && column
            .default_value
            .as_deref()
            .is_some_and(is_provably_non_null_literal)
}

fn is_provably_non_null_literal(default_value: &str) -> bool {
    let mut value = default_value.trim();
    while value.starts_with('(') && value.ends_with(')') {
        value = value[1..value.len() - 1].trim();
    }
    if value.is_empty() || value.eq_ignore_ascii_case("NULL") {
        return false;
    }
    if matches!(
        value.to_ascii_uppercase().as_str(),
        "TRUE" | "FALSE" | "CURRENT_DATE" | "CURRENT_TIME" | "CURRENT_TIMESTAMP"
    ) {
        return true;
    }
    if is_quoted_literal(value) {
        return true;
    }
    if value.len() >= 3
        && matches!(value.as_bytes()[0], b'x' | b'X')
        && is_quoted_literal(&value[1..])
    {
        let hex = &value[2..value.len() - 1];
        return hex.len().is_multiple_of(2) && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    let numeric = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    if let Some(hex) = numeric
        .strip_prefix("0x")
        .or_else(|| numeric.strip_prefix("0X"))
    {
        return !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    numeric.bytes().any(|byte| byte.is_ascii_digit())
        && numeric
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-'))
        && value.parse::<f64>().is_ok()
}

fn is_quoted_literal(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'\'' || bytes[bytes.len() - 1] != b'\'' {
        return false;
    }
    let mut index = 1;
    while index < bytes.len() - 1 {
        if bytes[index] == b'\'' {
            if index + 1 >= bytes.len() - 1 || bytes[index + 1] != b'\'' {
                return false;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    true
}

fn has_workspace_session_cascade(
    transaction: &Transaction<'_>,
    table: &str,
) -> Result<bool, MigrationError> {
    let foreign_keys = foreign_keys(transaction, table)?;
    Ok(foreign_keys.len() == 2
        && foreign_keys[0].id == foreign_keys[1].id
        && foreign_keys[0].sequence == 0
        && foreign_keys[1].sequence == 1
        && foreign_keys.iter().all(|foreign_key| {
            foreign_key.parent_table == "sessions"
                && foreign_key.on_update.eq_ignore_ascii_case("NO ACTION")
                && foreign_key.on_delete.eq_ignore_ascii_case("CASCADE")
                && foreign_key.match_rule.eq_ignore_ascii_case("NONE")
        })
        && foreign_keys[0].from == "workspace_id"
        && foreign_keys[0].to == "workspace_id"
        && foreign_keys[1].from == "session_id"
        && foreign_keys[1].to == "session_id")
}

#[derive(Debug, PartialEq, Eq)]
struct ForeignKey {
    id: i64,
    sequence: i64,
    parent_table: String,
    from: String,
    to: String,
    on_update: String,
    on_delete: String,
    match_rule: String,
}

fn foreign_keys(
    transaction: &Transaction<'_>,
    table: &str,
) -> Result<Vec<ForeignKey>, MigrationError> {
    let mut statement = transaction.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
    let mut foreign_keys = statement
        .query_map([], |row| {
            Ok(ForeignKey {
                id: row.get(0)?,
                sequence: row.get(1)?,
                parent_table: row.get(2)?,
                from: row.get(3)?,
                to: row.get(4)?,
                on_update: row.get(5)?,
                on_delete: row.get(6)?,
                match_rule: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    foreign_keys.sort_by_key(|foreign_key| (foreign_key.id, foreign_key.sequence));
    Ok(foreign_keys)
}

#[derive(Clone, Copy)]
struct UniqueIndexSpec {
    origin: &'static str,
    columns: &'static [&'static str],
}

impl UniqueIndexSpec {
    const fn new(origin: &'static str, columns: &'static [&'static str]) -> Self {
        Self { origin, columns }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UniqueIndex {
    origin: String,
    partial: bool,
    columns: Vec<String>,
}

fn validate_unique_indexes(
    transaction: &Transaction<'_>,
    table: &str,
    expected: &[UniqueIndexSpec],
) -> Result<(), MigrationError> {
    let mut statement = transaction.prepare(&format!("PRAGMA index_list({table})"))?;
    let indexes = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut actual = Vec::new();
    for (index, unique, origin, partial) in indexes {
        if !unique {
            continue;
        }
        let quoted = index.replace('"', "\"\"");
        let mut index_statement =
            transaction.prepare(&format!("PRAGMA index_xinfo(\"{quoted}\")"))?;
        let mut columns = index_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, bool>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        columns.retain(|(_, _, _, _, key)| *key);
        columns.sort_by_key(|(sequence, _, _, _, _)| *sequence);
        let mut names = Vec::with_capacity(columns.len());
        for (_, name, descending, collation, _) in columns {
            let Some(name) = name else {
                return Err(MigrationError::SchemaIntegrity {
                    reason: format!("{table} has an expression-based unique index"),
                });
            };
            if descending
                || !collation
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("BINARY"))
            {
                return Err(MigrationError::SchemaIntegrity {
                    reason: format!("{table} has an incompatible unique index"),
                });
            }
            names.push(name);
        }
        actual.push(UniqueIndex {
            origin,
            partial,
            columns: names,
        });
    }
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|index| UniqueIndex {
            origin: index.origin.to_owned(),
            partial: false,
            columns: index
                .columns
                .iter()
                .map(|column| (*column).to_owned())
                .collect(),
        })
        .collect::<Vec<_>>();
    expected.sort();
    if actual != expected {
        return Err(MigrationError::SchemaIntegrity {
            reason: format!("{table} has unexpected unique index topology"),
        });
    }
    Ok(())
}

fn validate_current_rows(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    let mut sessions = transaction
        .prepare("SELECT workspace_id, session_id, title, language, started_at_ms FROM sessions")?;
    for session in sessions.query_map([], |row| {
        Ok(NewSession {
            workspace_id: row.get(0)?,
            session_id: row.get(1)?,
            title: row.get(2)?,
            language: row.get(3)?,
            started_at_ms: row.get(4)?,
        })
    })? {
        session?
            .validate()
            .map_err(|error| MigrationError::SchemaIntegrity {
                reason: format!("sessions contains an invalid row: {error}"),
            })?;
    }

    let mut briefs = transaction
        .prepare("SELECT workspace_id, session_id, summary, updated_at_ms FROM session_briefs")?;
    for brief in briefs.query_map([], |row| {
        Ok(NewSessionBrief {
            workspace_id: row.get(0)?,
            session_id: row.get(1)?,
            summary: row.get(2)?,
            updated_at_ms: row.get(3)?,
        })
    })? {
        brief?
            .validate()
            .map_err(|error| MigrationError::SchemaIntegrity {
                reason: format!("session_briefs contains an invalid row: {error}"),
            })?;
    }

    let mut events = transaction.prepare(
        "SELECT workspace_id, session_id, event_id, source_generation, source_sequence,
                timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json
         FROM timeline_events",
    )?;
    for event in events.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, String>(10)?,
        ))
    })? {
        let (
            workspace_id,
            session_id,
            event_id,
            source_generation,
            source_sequence,
            timestamp_ms,
            kind,
            correlation_id,
            request_id,
            turn_id,
            payload_json,
        ) = event?;
        let kind =
            TimelineEventKind::from_db(&kind).map_err(|error| MigrationError::SchemaIntegrity {
                reason: format!("timeline_events contains an invalid row: {error}"),
            })?;
        if !kind.is_durable() {
            return Err(MigrationError::SchemaIntegrity {
                reason: "timeline_events contains a non-durable event".to_owned(),
            });
        }
        let payload = serde_json::from_str(&payload_json).map_err(|error| {
            MigrationError::SchemaIntegrity {
                reason: format!("timeline_events contains invalid JSON: {error}"),
            }
        })?;
        NewTimelineEvent {
            workspace_id,
            session_id,
            event_id,
            source_generation,
            source_sequence,
            timestamp_ms,
            kind,
            correlation_id,
            request_id,
            turn_id,
            payload,
        }
        .validate()
        .map_err(|error| MigrationError::SchemaIntegrity {
            reason: format!("timeline_events contains an invalid row: {error}"),
        })?;
    }

    let mut settings = transaction.prepare("SELECT workspace_id FROM settings")?;
    for workspace_id in settings.query_map([], |row| row.get::<_, String>(0))? {
        if workspace_id?.is_empty() {
            return Err(MigrationError::SchemaIntegrity {
                reason: "settings contains an empty workspace id".to_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{has_workspace_session_cascade, migrate, MigrationError, SCHEMA_VERSION};
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

    #[test]
    fn partial_timeline_uniqueness_does_not_satisfy_current_schema() {
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
                    request_id TEXT, turn_id TEXT, payload_json TEXT NOT NULL,
                    FOREIGN KEY (workspace_id, session_id)
                        REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
                );
                CREATE UNIQUE INDEX partial_timeline_order
                    ON timeline_events(workspace_id, session_id, host_sequence)
                    WHERE host_sequence > 0;",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn nullable_ownership_column_does_not_satisfy_current_schema() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT, setting_key TEXT NOT NULL, value_json TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, setting_key)
                );",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn unknown_lower_schema_version_fails_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection.pragma_update(None, "user_version", -1).unwrap();

        assert!(matches!(
            migrate(&mut connection),
            Err(MigrationError::UnsupportedSchemaVersion { found: -1 })
        ));
    }

    #[test]
    fn existing_foreign_key_violations_fail_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                INSERT INTO timeline_events (
                    event_id, workspace_id, session_id, host_sequence, source_generation,
                    source_sequence, timestamp_ms, kind, payload_json
                ) VALUES ('orphan', 'workspace', 'missing', 1, 1, 1, 1, 'note', '{}');",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn oversized_composite_foreign_key_is_not_accepted_as_ownership_cascade() {
        let mut connection = Connection::open_in_memory().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(
                "CREATE TABLE sessions (
                    workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, tenant_id TEXT,
                    PRIMARY KEY (workspace_id, session_id),
                    UNIQUE (workspace_id, session_id, tenant_id)
                );
                CREATE TABLE timeline_events (
                    workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, tenant_id TEXT,
                    FOREIGN KEY (workspace_id, session_id, tenant_id)
                        REFERENCES sessions(workspace_id, session_id, tenant_id)
                        ON DELETE CASCADE
                );",
            )
            .unwrap();

        assert!(!has_workspace_session_cascade(&transaction, "timeline_events").unwrap());
    }

    #[test]
    fn current_schema_rejects_nocase_workspace_ownership() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT COLLATE NOCASE NOT NULL,
                    setting_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, setting_key)
                );",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn current_schema_rejects_extra_required_column() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT NOT NULL,
                    setting_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    tenant_id TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, setting_key)
                );",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn extension_column_rejects_not_null_default_null() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT NOT NULL,
                    setting_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    tenant_id TEXT NOT NULL DEFAULT NULL,
                    PRIMARY KEY (workspace_id, setting_key)
                );",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn extension_column_rejects_not_null_generated_null() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT NOT NULL,
                    setting_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    tenant_id TEXT GENERATED ALWAYS AS (NULL) VIRTUAL NOT NULL,
                    PRIMARY KEY (workspace_id, setting_key)
                );",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn extension_column_accepts_nullable_column() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT NOT NULL,
                    setting_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    tenant_id TEXT,
                    PRIMARY KEY (workspace_id, setting_key)
                );",
            )
            .unwrap();

        migrate(&mut connection).unwrap();
    }

    #[test]
    fn extension_column_accepts_not_null_literal_default() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT NOT NULL,
                    setting_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    tenant_id TEXT NOT NULL DEFAULT 'local',
                    PRIMARY KEY (workspace_id, setting_key)
                );",
            )
            .unwrap();

        migrate(&mut connection).unwrap();
    }

    #[test]
    fn current_schema_rejects_extra_uniqueness() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "CREATE UNIQUE INDEX settings_value_unique
                    ON settings(workspace_id, value_json);",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn current_schema_rejects_malformed_extra_foreign_key() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "DROP TABLE settings;
                CREATE TABLE settings (
                    workspace_id TEXT NOT NULL,
                    setting_key TEXT NOT NULL,
                    value_json TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, setting_key),
                    FOREIGN KEY (workspace_id, setting_key)
                        REFERENCES sessions(workspace_id, session_id)
                        ON DELETE CASCADE
                );",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn current_schema_rejects_rows_with_empty_session_identity_or_language() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute(
                "INSERT INTO sessions (
                    workspace_id, session_id, title, language, started_at_ms
                ) VALUES ('', '', NULL, '', 1)",
                [],
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }
}
