use std::collections::{btree_map::Entry, BTreeMap};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::models::{
    NewSession, NewSessionBrief, NewTimelineEvent, RequestTurnAssociation, SessionStatus,
    TimelineEventKind,
};

pub const SCHEMA_VERSION: i64 = 2;

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
    if !(0..=SCHEMA_VERSION).contains(&version) {
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
        create_v2_schema(&transaction)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    } else if version == 1 {
        migrate_v1_to_v2(&transaction)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    validate_current_schema(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn create_v2_schema(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    transaction.execute_batch(
        "CREATE TABLE sessions (
            workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, mode TEXT NOT NULL, status TEXT NOT NULL,
            ui_language TEXT, input_language TEXT NOT NULL, response_language TEXT NOT NULL,
            review_language TEXT NOT NULL, started_at_ms INTEGER NOT NULL, completed_at_ms INTEGER,
            PRIMARY KEY (workspace_id, session_id)
        );
        CREATE TABLE session_briefs (
            workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, brief_json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL, PRIMARY KEY (workspace_id, session_id),
            FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
        );
        CREATE TABLE timeline_events (
            event_id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL, session_id TEXT NOT NULL,
            host_sequence INTEGER NOT NULL, source_generation INTEGER NOT NULL, source_sequence INTEGER NOT NULL,
            timestamp_ms INTEGER NOT NULL, kind TEXT NOT NULL, correlation_id TEXT, request_id TEXT, turn_id TEXT,
            payload_json TEXT NOT NULL, UNIQUE (workspace_id, session_id, host_sequence),
            FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
        );
        CREATE TABLE request_turn_associations (
            workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, request_id TEXT NOT NULL, turn_id TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL, PRIMARY KEY (workspace_id, session_id, request_id),
            FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
        );
        CREATE TABLE settings (
            workspace_id TEXT NOT NULL, setting_key TEXT NOT NULL, value_json TEXT NOT NULL,
            PRIMARY KEY (workspace_id, setting_key)
        );",
    )?;
    Ok(())
}

fn migrate_v1_to_v2(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    validate_v1_schema(transaction)?;
    let sessions = {
        let mut statement = transaction.prepare(
            "SELECT workspace_id, session_id, title, language, started_at_ms, completed_at_ms FROM sessions",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let briefs = {
        let mut statement = transaction.prepare(
            "SELECT workspace_id, session_id, summary, updated_at_ms FROM session_briefs",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let mut legacy_briefs = briefs
        .into_iter()
        .map(|(workspace_id, session_id, summary, updated_at_ms)| {
            ((workspace_id, session_id), (summary, updated_at_ms))
        })
        .collect::<BTreeMap<_, _>>();
    let events = {
        let mut statement = transaction.prepare("SELECT event_id, workspace_id, session_id, host_sequence, source_generation, source_sequence, timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json FROM timeline_events")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    let mut associations = BTreeMap::new();
    for event in &events {
        let (Some(request_id), Some(turn_id)) = (&event.9, &event.10) else {
            continue;
        };
        if request_id.is_empty() || turn_id.is_empty() {
            continue;
        }
        match associations.entry((event.1.clone(), event.2.clone(), request_id.clone())) {
            Entry::Vacant(entry) => {
                entry.insert((turn_id.clone(), event.6));
            }
            Entry::Occupied(mut entry) => {
                let (existing_turn_id, earliest_timestamp_ms) = entry.get_mut();
                if existing_turn_id != turn_id {
                    return Err(MigrationError::SchemaIntegrity {
                        reason: "timeline_events contains conflicting request-to-turn associations"
                            .to_owned(),
                    });
                }
                *earliest_timestamp_ms = (*earliest_timestamp_ms).min(event.6);
            }
        }
    }

    transaction.execute_batch(
        "ALTER TABLE session_briefs RENAME TO session_briefs_v1;
         ALTER TABLE timeline_events RENAME TO timeline_events_v1;
         ALTER TABLE sessions RENAME TO sessions_v1;",
    )?;
    create_v2_schema_without_settings(transaction)?;
    for (workspace_id, session_id, title, language, started_at_ms, completed_at_ms) in sessions {
        let status = if completed_at_ms.is_some() {
            SessionStatus::Completed
        } else {
            SessionStatus::Active
        };
        transaction.execute(
            "INSERT INTO sessions (workspace_id, session_id, mode, status, ui_language, input_language, response_language, review_language, started_at_ms, completed_at_ms) VALUES (?1, ?2, 'interview', ?3, NULL, ?4, ?4, ?4, ?5, ?6)",
            rusqlite::params![workspace_id, session_id, status.as_db(), language, started_at_ms, completed_at_ms],
        )?;
        let legacy_brief = legacy_briefs.remove(&(workspace_id.clone(), session_id.clone()));
        let migrated_brief = match (title, legacy_brief) {
            (Some(title), Some((summary, updated_at_ms))) => Some((
                serde_json::json!({ "summary": summary, "title": title }),
                updated_at_ms,
            )),
            (Some(title), None) => Some((serde_json::json!({ "title": title }), started_at_ms)),
            (None, Some((summary, updated_at_ms))) => {
                Some((serde_json::json!({ "summary": summary }), updated_at_ms))
            }
            (None, None) => None,
        };
        if let Some((brief, updated_at_ms)) = migrated_brief {
            let brief_json =
                serde_json::to_string(&brief).map_err(|error| MigrationError::SchemaIntegrity {
                    reason: error.to_string(),
                })?;
            transaction.execute(
                "INSERT INTO session_briefs (workspace_id, session_id, brief_json, updated_at_ms) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![workspace_id, session_id, brief_json, updated_at_ms],
            )?;
        }
    }
    for (
        event_id,
        workspace_id,
        session_id,
        host_sequence,
        source_generation,
        source_sequence,
        timestamp_ms,
        kind,
        correlation_id,
        request_id,
        turn_id,
        payload_json,
    ) in events
    {
        transaction.execute(
            "INSERT INTO timeline_events (event_id, workspace_id, session_id, host_sequence, source_generation, source_sequence, timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![event_id, workspace_id, session_id, host_sequence, source_generation, source_sequence, timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json],
        )?;
    }
    for ((workspace_id, session_id, request_id), (turn_id, created_at_ms)) in associations {
        transaction.execute(
            "INSERT INTO request_turn_associations (workspace_id, session_id, request_id, turn_id, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![workspace_id, session_id, request_id, turn_id, created_at_ms],
        )?;
    }
    transaction.execute_batch(
        "DROP TABLE session_briefs_v1;
         DROP TABLE timeline_events_v1;
         DROP TABLE sessions_v1;",
    )?;
    Ok(())
}

fn create_v2_schema_without_settings(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    transaction.execute_batch(
        "CREATE TABLE sessions (
            workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, mode TEXT NOT NULL, status TEXT NOT NULL,
            ui_language TEXT, input_language TEXT NOT NULL, response_language TEXT NOT NULL,
            review_language TEXT NOT NULL, started_at_ms INTEGER NOT NULL, completed_at_ms INTEGER,
            PRIMARY KEY (workspace_id, session_id)
        );
        CREATE TABLE session_briefs (
            workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, brief_json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL, PRIMARY KEY (workspace_id, session_id),
            FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
        );
        CREATE TABLE timeline_events (
            event_id TEXT PRIMARY KEY NOT NULL, workspace_id TEXT NOT NULL, session_id TEXT NOT NULL,
            host_sequence INTEGER NOT NULL, source_generation INTEGER NOT NULL, source_sequence INTEGER NOT NULL,
            timestamp_ms INTEGER NOT NULL, kind TEXT NOT NULL, correlation_id TEXT, request_id TEXT, turn_id TEXT,
            payload_json TEXT NOT NULL, UNIQUE (workspace_id, session_id, host_sequence),
            FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
        );
        CREATE TABLE request_turn_associations (
            workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, request_id TEXT NOT NULL, turn_id TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL, PRIMARY KEY (workspace_id, session_id, request_id),
            FOREIGN KEY (workspace_id, session_id) REFERENCES sessions(workspace_id, session_id) ON DELETE CASCADE
        );",
    )?;
    Ok(())
}

fn validate_current_schema(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    validate_table(
        transaction,
        "sessions",
        &[
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("session_id", "TEXT", true, 2).with_binary_collation(),
            ColumnSpec::new("mode", "TEXT", true, 0),
            ColumnSpec::new("status", "TEXT", true, 0),
            ColumnSpec::new("ui_language", "TEXT", false, 0),
            ColumnSpec::new("input_language", "TEXT", true, 0),
            ColumnSpec::new("response_language", "TEXT", true, 0),
            ColumnSpec::new("review_language", "TEXT", true, 0),
            ColumnSpec::new("started_at_ms", "INTEGER", true, 0),
            ColumnSpec::new("completed_at_ms", "INTEGER", false, 0),
        ],
        false,
    )?;
    validate_table(
        transaction,
        "session_briefs",
        &[
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("session_id", "TEXT", true, 2).with_binary_collation(),
            ColumnSpec::new("brief_json", "TEXT", true, 0),
            ColumnSpec::new("updated_at_ms", "INTEGER", true, 0),
        ],
        false,
    )?;
    validate_table(
        transaction,
        "request_turn_associations",
        &[
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("session_id", "TEXT", true, 2).with_binary_collation(),
            ColumnSpec::new("request_id", "TEXT", true, 3),
            ColumnSpec::new("turn_id", "TEXT", true, 0),
            ColumnSpec::new("created_at_ms", "INTEGER", true, 0),
        ],
        false,
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
        false,
    )?;
    validate_table(
        transaction,
        "settings",
        &[
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("setting_key", "TEXT", true, 2),
            ColumnSpec::new("value_json", "TEXT", true, 0),
        ],
        false,
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
        "request_turn_associations",
        &[UniqueIndexSpec::new(
            "pk",
            &["workspace_id", "session_id", "request_id"],
        )],
    )?;
    validate_unique_indexes(
        transaction,
        "settings",
        &[UniqueIndexSpec::new("pk", &["workspace_id", "setting_key"])],
    )?;
    validate_no_owned_table_triggers(
        transaction,
        &[
            "sessions",
            "session_briefs",
            "timeline_events",
            "request_turn_associations",
            "settings",
        ],
    )?;

    for table in ["sessions", "settings"] {
        if !foreign_keys(transaction, table)?.is_empty() {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!("{table} has unexpected foreign keys"),
            });
        }
    }
    for table in [
        "session_briefs",
        "timeline_events",
        "request_turn_associations",
    ] {
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

fn validate_v1_schema(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
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
        true,
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
        true,
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
        true,
    )?;
    validate_table(
        transaction,
        "settings",
        &[
            ColumnSpec::new("workspace_id", "TEXT", true, 1).with_binary_collation(),
            ColumnSpec::new("setting_key", "TEXT", true, 2),
            ColumnSpec::new("value_json", "TEXT", true, 0),
        ],
        true,
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
    validate_no_owned_table_triggers(
        transaction,
        &["sessions", "session_briefs", "timeline_events", "settings"],
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
                reason: format!("{table} must have an ownership cascade"),
            });
        }
    }
    let mut foreign_key_check = transaction.prepare("PRAGMA foreign_key_check")?;
    if foreign_key_check.query([])?.next()?.is_some() {
        return Err(MigrationError::SchemaIntegrity {
            reason: "the database contains rows that violate ownership relationships".to_owned(),
        });
    }
    validate_v1_rows(transaction)
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
    reject_extra_columns: bool,
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
        if !required && (reject_extra_columns || !extra_column_is_insert_compatible(column)) {
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

fn validate_no_owned_table_triggers(
    transaction: &Transaction<'_>,
    tables: &[&str],
) -> Result<(), MigrationError> {
    for table in tables {
        let has_trigger: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?1
                UNION ALL
                SELECT 1 FROM sqlite_temp_master WHERE type = 'trigger' AND tbl_name = ?1
            )",
            [table],
            |row| row.get(0),
        )?;
        if has_trigger {
            return Err(MigrationError::SchemaIntegrity {
                reason: format!("{table} has unexpected triggers"),
            });
        }
    }
    Ok(())
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

fn validate_v1_rows(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    let mut sessions =
        transaction.prepare("SELECT workspace_id, session_id, language FROM sessions")?;
    for row in sessions.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (workspace_id, session_id, language) = row?;
        if workspace_id.is_empty() || session_id.is_empty() || language.is_empty() {
            return Err(MigrationError::SchemaIntegrity {
                reason: "sessions contains an invalid row".to_owned(),
            });
        }
    }
    let mut briefs = transaction.prepare("SELECT workspace_id, session_id FROM session_briefs")?;
    for row in briefs.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (workspace_id, session_id) = row?;
        if workspace_id.is_empty() || session_id.is_empty() {
            return Err(MigrationError::SchemaIntegrity {
                reason: "session_briefs contains an invalid row".to_owned(),
            });
        }
    }
    let mut events = transaction.prepare(
        "SELECT workspace_id, session_id, event_id, source_generation, source_sequence,
                timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json
         FROM timeline_events",
    )?;
    for event in events.query_map([], |row| {
        let kind: String = row.get(6)?;
        let payload_json: String = row.get(10)?;
        Ok(NewTimelineEvent {
            workspace_id: row.get(0)?,
            session_id: row.get(1)?,
            event_id: row.get(2)?,
            source_generation: row.get(3)?,
            source_sequence: row.get(4)?,
            timestamp_ms: row.get(5)?,
            kind: TimelineEventKind::from_db(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
            correlation_id: row.get(7)?,
            request_id: row.get(8)?,
            turn_id: row.get(9)?,
            payload: serde_json::from_str(&payload_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        })
    })? {
        let event = event?;
        if !event.kind.is_durable() || event.validate().is_err() {
            return Err(MigrationError::SchemaIntegrity {
                reason: "timeline_events contains an invalid row".to_owned(),
            });
        }
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

fn validate_current_rows(transaction: &Transaction<'_>) -> Result<(), MigrationError> {
    let mut sessions = transaction.prepare(
        "SELECT workspace_id, session_id, mode, status, ui_language, input_language,
                response_language, review_language, started_at_ms, completed_at_ms FROM sessions",
    )?;
    for session in sessions.query_map([], |row| {
        Ok(NewSession {
            workspace_id: row.get(0)?,
            session_id: row.get(1)?,
            mode: row.get(2)?,
            status: SessionStatus::from_db(&row.get::<_, String>(3)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            ui_language: row.get(4)?,
            input_language: row.get(5)?,
            response_language: row.get(6)?,
            review_language: row.get(7)?,
            started_at_ms: row.get(8)?,
            completed_at_ms: row.get(9)?,
        })
    })? {
        session?
            .validate()
            .map_err(|error| MigrationError::SchemaIntegrity {
                reason: format!("sessions contains an invalid row: {error}"),
            })?;
    }

    let mut briefs = transaction.prepare(
        "SELECT workspace_id, session_id, brief_json, updated_at_ms FROM session_briefs",
    )?;
    for brief in briefs.query_map([], |row| {
        Ok(NewSessionBrief {
            workspace_id: row.get(0)?,
            session_id: row.get(1)?,
            brief: serde_json::from_str(&row.get::<_, String>(2)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
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

    let mut associations = transaction.prepare(
        "SELECT workspace_id, session_id, request_id, turn_id, created_at_ms
         FROM request_turn_associations",
    )?;
    for association in associations.query_map([], |row| {
        Ok(RequestTurnAssociation {
            workspace_id: row.get(0)?,
            session_id: row.get(1)?,
            request_id: row.get(2)?,
            turn_id: row.get(3)?,
            created_at_ms: row.get(4)?,
        })
    })? {
        association?
            .validate()
            .map_err(|error| MigrationError::SchemaIntegrity {
                reason: format!("request_turn_associations contains an invalid row: {error}"),
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

    fn create_v1_schema(connection: &Connection) {
        connection
            .execute_batch(
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
                );",
            )
            .unwrap();
    }

    #[test]
    fn empty_database_migrates_to_current_owned_schema() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        for table in [
            "sessions",
            "session_briefs",
            "timeline_events",
            "request_turn_associations",
            "settings",
        ] {
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
    fn owned_v1_schema_migrates_to_v2_without_losing_sessions_briefs_or_timeline() {
        let mut connection = Connection::open_in_memory().unwrap();
        create_v1_schema(&connection);
        connection
            .execute_batch(
                "INSERT INTO sessions VALUES
                    ('workspace-a', 'active', 'legacy active', 'en-US', 100, NULL),
                    ('workspace-a', 'completed', 'legacy completed', 'ur-PK', 200, 300),
                    ('workspace-a', 'empty-title', '', 'en-US', 250, NULL);
                INSERT INTO session_briefs VALUES ('workspace-a', 'active', 'legacy brief', 400);
                INSERT INTO timeline_events VALUES
                    ('event-a', 'workspace-a', 'active', 1, 2, 3, 500, 'note', NULL, NULL, NULL, '{\"note\":true}'),
                    ('assoc-late', 'workspace-a', 'active', 2, 2, 4, 700, 'note', NULL, 'request-a', 'turn-a', '{}'),
                    ('assoc-early', 'workspace-a', 'active', 3, 2, 5, 600, 'note', NULL, 'request-a', 'turn-a', '{}');
                PRAGMA user_version = 1;",
            )
            .unwrap();

        migrate(&mut connection).unwrap();

        assert_eq!(
            connection
                .query_row(
                    "SELECT mode, status, ui_language, input_language, response_language, review_language, completed_at_ms
                     FROM sessions WHERE workspace_id = 'workspace-a' AND session_id = 'active'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?, row.get::<_, Option<i64>>(6)?)),
                )
                .unwrap(),
            ("interview".into(), "active".into(), None, "en-US".into(), "en-US".into(), "en-US".into(), None)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT status, completed_at_ms FROM sessions WHERE workspace_id = 'workspace-a' AND session_id = 'completed'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
                )
                .unwrap(),
            ("completed".into(), Some(300))
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT brief_json FROM session_briefs WHERE workspace_id = 'workspace-a' AND session_id = 'active'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "{\"summary\":\"legacy brief\",\"title\":\"legacy active\"}"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT brief_json, updated_at_ms FROM session_briefs
                     WHERE workspace_id = 'workspace-a' AND session_id = 'completed'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("{\"title\":\"legacy completed\"}".into(), 200)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT brief_json, updated_at_ms FROM session_briefs
                     WHERE workspace_id = 'workspace-a' AND session_id = 'empty-title'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("{\"title\":\"\"}".into(), 250)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload_json FROM timeline_events WHERE event_id = 'event-a'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "{\"note\":true}"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT turn_id, created_at_ms FROM request_turn_associations
                     WHERE workspace_id = 'workspace-a' AND session_id = 'active' AND request_id = 'request-a'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("turn-a".into(), 600)
        );
    }

    #[test]
    fn v1_migration_rejects_insert_compatible_extension_columns_without_mutation() {
        let mut connection = Connection::open_in_memory().unwrap();
        create_v1_schema(&connection);
        connection
            .execute_batch(
                "ALTER TABLE sessions ADD COLUMN extension_value TEXT;
                 PRAGMA user_version = 1;",
            )
            .unwrap();

        assert!(matches!(
            migrate(&mut connection),
            Err(MigrationError::SchemaIntegrity { .. })
        ));
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(connection
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .any(|column| column.unwrap() == "extension_value"));
    }

    #[test]
    fn conflicting_v1_request_turn_rows_roll_back_the_migration() {
        let mut connection = Connection::open_in_memory().unwrap();
        create_v1_schema(&connection);
        connection
            .execute_batch(
                "INSERT INTO sessions VALUES
                    ('workspace-a', 'session-a', 'legacy', 'en', 100, NULL);
                 INSERT INTO timeline_events VALUES
                    ('event-a', 'workspace-a', 'session-a', 1, 1, 1, 200, 'note', NULL, 'request-a', 'turn-a', '{}'),
                    ('event-b', 'workspace-a', 'session-a', 2, 1, 2, 300, 'note', NULL, 'request-a', 'turn-b', '{}');
                 PRAGMA user_version = 1;",
            )
            .unwrap();

        assert!(matches!(
            migrate(&mut connection),
            Err(MigrationError::SchemaIntegrity { .. })
        ));
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT title FROM sessions WHERE session_id = 'session-a'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "legacy"
        );
        assert!(!connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'request_turn_associations')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
    }

    #[test]
    fn v1_and_v2_schemas_reject_destructive_owned_table_triggers() {
        let mut v1 = Connection::open_in_memory().unwrap();
        create_v1_schema(&v1);
        v1.execute_batch(
            "CREATE TRIGGER destroy_v1_settings AFTER INSERT ON sessions
             BEGIN DELETE FROM settings; END;
             PRAGMA user_version = 1;",
        )
        .unwrap();
        assert!(matches!(
            migrate(&mut v1),
            Err(MigrationError::SchemaIntegrity { .. })
        ));

        let mut v2 = Connection::open_in_memory().unwrap();
        migrate(&mut v2).unwrap();
        v2.execute_batch(
            "CREATE TRIGGER destroy_v2_sessions AFTER INSERT ON request_turn_associations
             BEGIN DELETE FROM sessions; END;",
        )
        .unwrap();
        assert!(matches!(
            migrate(&mut v2),
            Err(MigrationError::SchemaIntegrity { .. })
        ));
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
                    workspace_id, session_id, mode, status, ui_language, input_language,
                    response_language, review_language, started_at_ms, completed_at_ms
                ) VALUES ('', '', '', 'active', NULL, '', '', '', 1, NULL)",
                [],
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn current_schema_rejects_invalid_json_briefs() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO sessions (
                    workspace_id, session_id, mode, status, ui_language, input_language,
                    response_language, review_language, started_at_ms, completed_at_ms
                ) VALUES ('workspace-a', 'session-a', 'interview', 'active', NULL, 'en', 'en', 'en', 1, NULL);
                INSERT INTO session_briefs (workspace_id, session_id, brief_json, updated_at_ms)
                VALUES ('workspace-a', 'session-a', '{not valid json}', 2);",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }

    #[test]
    fn current_schema_rejects_associations_with_empty_ids() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO sessions (
                    workspace_id, session_id, mode, status, ui_language, input_language,
                    response_language, review_language, started_at_ms, completed_at_ms
                ) VALUES ('workspace-a', 'session-a', 'interview', 'active', NULL, 'en', 'en', 'en', 1, NULL);
                INSERT INTO request_turn_associations (
                    workspace_id, session_id, request_id, turn_id, created_at_ms
                ) VALUES ('workspace-a', 'session-a', '', 'turn-a', 2);",
            )
            .unwrap();

        assert!(migrate(&mut connection).is_err());
    }
}
