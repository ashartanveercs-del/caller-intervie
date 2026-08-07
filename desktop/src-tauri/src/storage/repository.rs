use std::{path::Path, sync::Mutex};

use rusqlite::{
    ffi, params, types::Type, Connection, OptionalExtension, Transaction, TransactionBehavior,
};
use serde_json::Value;

use super::{
    migrations::{migrate, MigrationError},
    AppendEventResult, AssociateRequestResult, ModelError, NewSession, NewSessionBrief,
    NewTimelineEvent, RequestTurnAssociation, SessionStatus, StoredSession, StoredSessionBrief,
    StoredTimelineEvent, TimelineEventKind,
};

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("SQLCipher is not available")]
    CipherUnavailable,
    #[error("database connection is unavailable")]
    ConnectionUnavailable,
    #[error("timeline event is transient and cannot be persisted: {0}")]
    NonDurableEvent(TimelineEventKind),
    #[error("timeline event id conflicts with existing durable content")]
    EventContentCollision { event_id: String },
    #[error("the requested workspace-owned session was not found")]
    NotFound,
    #[error("session list limit must be between 1 and 100")]
    InvalidSessionLimit { limit: usize },
    #[error("completed sessions must have completed or interrupted status")]
    InvalidCompletionStatus,
    #[error("session brief could not be encoded as JSON")]
    BriefSerialization,
    #[error("request id is already associated with a different turn")]
    RequestTurnAssociationConflict {
        workspace_id: String,
        session_id: String,
        request_id: String,
        existing_turn_id: String,
        requested_turn_id: String,
    },
    #[error(transparent)]
    Migration(#[from] MigrationError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
}

pub struct SessionRepository {
    connection: Mutex<Connection>,
}

impl SessionRepository {
    pub fn open(path: impl AsRef<Path>, key: &[u8]) -> Result<Self, RepositoryError> {
        if key.len() != 32 {
            return Err(RepositoryError::CipherUnavailable);
        }
        let mut connection = Connection::open(path)?;
        apply_cipher_key(&mut connection, key)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = DELETE;")?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn cipher_version(&self) -> Result<String, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let version: String =
            connection.query_row("PRAGMA cipher_version", [], |row| row.get(0))?;
        if version.trim().is_empty() {
            return Err(RepositoryError::CipherUnavailable);
        }
        Ok(version)
    }

    pub fn create_session(&self, session: &NewSession) -> Result<(), RepositoryError> {
        session.validate()?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        connection.execute("INSERT INTO sessions (workspace_id, session_id, mode, status, ui_language, input_language, response_language, review_language, started_at_ms, completed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)", params![session.workspace_id, session.session_id, session.mode, session.status.as_db(), session.ui_language, session.input_language, session.response_language, session.review_language, session.started_at_ms, session.completed_at_ms])?;
        Ok(())
    }

    pub fn save_session_brief(&self, brief: &NewSessionBrief) -> Result<(), RepositoryError> {
        brief.validate()?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let brief_json =
            serde_json::to_string(&brief.brief).map_err(|_| RepositoryError::BriefSerialization)?;
        let updated = connection.execute("INSERT INTO session_briefs (workspace_id, session_id, brief_json, updated_at_ms) SELECT workspace_id, session_id, ?3, ?4 FROM sessions WHERE workspace_id = ?1 AND session_id = ?2 ON CONFLICT(workspace_id, session_id) DO UPDATE SET brief_json = excluded.brief_json, updated_at_ms = excluded.updated_at_ms", params![brief.workspace_id, brief.session_id, brief_json, brief.updated_at_ms])?;
        if updated == 0 {
            return Err(RepositoryError::NotFound);
        }
        Ok(())
    }

    pub fn append_event(
        &self,
        event: &NewTimelineEvent,
    ) -> Result<AppendEventResult, RepositoryError> {
        event.validate()?;
        if !event.kind.is_durable() {
            return Err(RepositoryError::NonDurableEvent(event.kind.clone()));
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_session(&transaction, &event.workspace_id, &event.session_id)?;
        let payload_json = serde_json::to_string(&event.payload).map_err(|_| {
            RepositoryError::EventContentCollision {
                event_id: event.event_id.clone(),
            }
        })?;
        if let Some((host_sequence, existing)) = existing_event(&transaction, &event.event_id)? {
            if existing == event_content(event) {
                return Ok(AppendEventResult::Duplicate { host_sequence });
            }
            return Err(RepositoryError::EventContentCollision {
                event_id: event.event_id.clone(),
            });
        }
        let host_sequence: i64 = transaction.query_row("SELECT COALESCE(MAX(host_sequence), 0) + 1 FROM timeline_events WHERE workspace_id = ?1 AND session_id = ?2", params![event.workspace_id, event.session_id], |row| row.get(0))?;
        transaction.execute("INSERT INTO timeline_events (event_id, workspace_id, session_id, host_sequence, source_generation, source_sequence, timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)", params![event.event_id, event.workspace_id, event.session_id, host_sequence, event.source_generation, event.source_sequence, event.timestamp_ms, event.kind.as_db(), event.correlation_id, event.request_id, event.turn_id, payload_json])?;
        transaction.commit()?;
        Ok(AppendEventResult::Inserted { host_sequence })
    }

    pub fn complete_session(
        &self,
        workspace_id: &str,
        session_id: &str,
        status: SessionStatus,
        completed_at_ms: i64,
    ) -> Result<bool, RepositoryError> {
        if matches!(status, SessionStatus::Active) {
            return Err(RepositoryError::InvalidCompletionStatus);
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        Ok(connection.execute(
            "UPDATE sessions SET status = ?3, completed_at_ms = ?4 WHERE workspace_id = ?1 AND session_id = ?2",
            params![workspace_id, session_id, status.as_db(), completed_at_ms],
        )? > 0)
    }

    pub fn list_sessions(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredSession>, RepositoryError> {
        if !(1..=100).contains(&limit) {
            return Err(RepositoryError::InvalidSessionLimit { limit });
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let mut statement = connection.prepare(&session_query(
            "WHERE sessions.workspace_id = ?1 ORDER BY sessions.started_at_ms DESC LIMIT ?2",
        ))?;
        let sessions = statement
            .query_map(params![workspace_id, limit as i64], session_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(RepositoryError::from)?;
        Ok(sessions)
    }

    pub fn get_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredSession>, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        connection
            .query_row(
                &session_query("WHERE sessions.workspace_id = ?1 AND sessions.session_id = ?2"),
                params![workspace_id, session_id],
                session_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn get_timeline(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Vec<StoredTimelineEvent>, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let mut statement = connection.prepare("SELECT workspace_id, session_id, event_id, host_sequence, source_generation, source_sequence, timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json FROM timeline_events WHERE workspace_id = ?1 AND session_id = ?2 ORDER BY host_sequence")?;
        let timeline = statement
            .query_map(params![workspace_id, session_id], event_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(RepositoryError::from)?;
        Ok(timeline)
    }

    pub fn restore_active_session(
        &self,
        workspace_id: &str,
    ) -> Result<Option<StoredSession>, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        connection.query_row(&session_query("WHERE sessions.workspace_id = ?1 AND sessions.status = 'active' ORDER BY sessions.started_at_ms DESC LIMIT 1"), [workspace_id], session_from_row).optional().map_err(Into::into)
    }

    pub fn delete_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<bool, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        Ok(connection.execute(
            "DELETE FROM sessions WHERE workspace_id = ?1 AND session_id = ?2",
            params![workspace_id, session_id],
        )? > 0)
    }

    pub fn associate_request_with_turn(
        &self,
        association: &RequestTurnAssociation,
    ) -> Result<AssociateRequestResult, RepositoryError> {
        association.validate()?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_session(
            &transaction,
            &association.workspace_id,
            &association.session_id,
        )?;
        let existing_turn_id: Option<String> = transaction
            .query_row(
                "SELECT turn_id FROM request_turn_associations WHERE workspace_id = ?1 AND session_id = ?2 AND request_id = ?3",
                params![association.workspace_id, association.session_id, association.request_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing_turn_id) = existing_turn_id {
            if existing_turn_id == association.turn_id {
                return Ok(AssociateRequestResult::Duplicate);
            }
            return Err(RepositoryError::RequestTurnAssociationConflict {
                workspace_id: association.workspace_id.clone(),
                session_id: association.session_id.clone(),
                request_id: association.request_id.clone(),
                existing_turn_id,
                requested_turn_id: association.turn_id.clone(),
            });
        }
        transaction.execute(
            "INSERT INTO request_turn_associations (workspace_id, session_id, request_id, turn_id, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![association.workspace_id, association.session_id, association.request_id, association.turn_id, association.created_at_ms],
        )?;
        transaction.commit()?;
        Ok(AssociateRequestResult::Inserted)
    }

    pub fn resolve_request_turn(
        &self,
        workspace_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<String>, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        connection
            .query_row(
                "SELECT turn_id FROM request_turn_associations WHERE workspace_id = ?1 AND session_id = ?2 AND request_id = ?3",
                params![workspace_id, session_id, request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn get_request_turn_associations(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Vec<RequestTurnAssociation>, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let mut statement = connection.prepare("SELECT workspace_id, session_id, request_id, turn_id, created_at_ms FROM request_turn_associations WHERE workspace_id = ?1 AND session_id = ?2 ORDER BY created_at_ms, request_id")?;
        statement
            .query_map(params![workspace_id, session_id], association_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

fn apply_cipher_key(connection: &mut Connection, key: &[u8]) -> Result<(), RepositoryError> {
    let key_length = i32::try_from(key.len()).map_err(|_| RepositoryError::CipherUnavailable)?;
    let database_name = b"main\0";
    // The connection and byte slices remain valid and exclusively borrowed for the FFI call.
    let result = unsafe {
        ffi::sqlite3_key_v2(
            connection.handle(),
            database_name.as_ptr().cast(),
            key.as_ptr().cast(),
            key_length,
        )
    };
    if result != ffi::SQLITE_OK {
        return Err(rusqlite::Error::SqliteFailure(ffi::Error::new(result), None).into());
    }
    connection.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let version: String = connection.query_row("PRAGMA cipher_version", [], |row| row.get(0))?;
    if version.trim().is_empty() {
        return Err(RepositoryError::CipherUnavailable);
    }
    Ok(())
}

fn ensure_session(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    session_id: &str,
) -> Result<(), RepositoryError> {
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE workspace_id = ?1 AND session_id = ?2)",
        params![workspace_id, session_id],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::NotFound)
    }
}

#[derive(Debug, PartialEq)]
struct EventContent {
    workspace_id: String,
    session_id: String,
    source_generation: i64,
    source_sequence: i64,
    timestamp_ms: i64,
    kind: String,
    correlation_id: Option<String>,
    request_id: Option<String>,
    turn_id: Option<String>,
    payload: Value,
}

fn event_content(event: &NewTimelineEvent) -> EventContent {
    EventContent {
        workspace_id: event.workspace_id.clone(),
        session_id: event.session_id.clone(),
        source_generation: event.source_generation,
        source_sequence: event.source_sequence,
        timestamp_ms: event.timestamp_ms,
        kind: event.kind.as_db().to_owned(),
        correlation_id: event.correlation_id.clone(),
        request_id: event.request_id.clone(),
        turn_id: event.turn_id.clone(),
        payload: event.payload.clone(),
    }
}

fn existing_event(
    transaction: &Transaction<'_>,
    event_id: &str,
) -> Result<Option<(i64, EventContent)>, RepositoryError> {
    transaction.query_row("SELECT host_sequence, workspace_id, session_id, source_generation, source_sequence, timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json FROM timeline_events WHERE event_id = ?1", [event_id], |row| {
        let payload_json: String = row.get(10)?;
        let content = EventContent {
            workspace_id: row.get(1)?,
            session_id: row.get(2)?,
            source_generation: row.get(3)?,
            source_sequence: row.get(4)?,
            timestamp_ms: row.get(5)?,
            kind: row.get(6)?,
            correlation_id: row.get(7)?,
            request_id: row.get(8)?,
            turn_id: row.get(9)?,
            payload: serde_json::from_str(&payload_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(error))
            })?,
        };
        Ok((row.get(0)?, content))
    }).optional().map_err(Into::into)
}

fn session_query(suffix: &str) -> String {
    format!(
        "SELECT sessions.workspace_id, sessions.session_id, sessions.mode, sessions.status,
                sessions.ui_language, sessions.input_language, sessions.response_language,
                sessions.review_language, sessions.started_at_ms, sessions.completed_at_ms,
                session_briefs.brief_json, session_briefs.updated_at_ms
         FROM sessions LEFT JOIN session_briefs
           ON session_briefs.workspace_id = sessions.workspace_id
          AND session_briefs.session_id = sessions.session_id {suffix}"
    )
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSession> {
    let status: String = row.get(3)?;
    let brief_json: Option<String> = row.get(10)?;
    let brief = brief_json
        .map(|brief_json| {
            Ok(StoredSessionBrief {
                brief: serde_json::from_str(&brief_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(error))
                })?,
                updated_at_ms: row.get(11)?,
            })
        })
        .transpose()?;
    Ok(StoredSession {
        workspace_id: row.get(0)?,
        session_id: row.get(1)?,
        mode: row.get(2)?,
        status: SessionStatus::from_db(&status).map_err(|_| rusqlite::Error::InvalidQuery)?,
        ui_language: row.get(4)?,
        input_language: row.get(5)?,
        response_language: row.get(6)?,
        review_language: row.get(7)?,
        started_at_ms: row.get(8)?,
        completed_at_ms: row.get(9)?,
        brief,
    })
}

fn association_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestTurnAssociation> {
    Ok(RequestTurnAssociation {
        workspace_id: row.get(0)?,
        session_id: row.get(1)?,
        request_id: row.get(2)?,
        turn_id: row.get(3)?,
        created_at_ms: row.get(4)?,
    })
}
fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredTimelineEvent> {
    let kind: String = row.get(7)?;
    let payload: String = row.get(11)?;
    Ok(StoredTimelineEvent {
        workspace_id: row.get(0)?,
        session_id: row.get(1)?,
        event_id: row.get(2)?,
        host_sequence: row.get(3)?,
        source_generation: row.get(4)?,
        source_sequence: row.get(5)?,
        timestamp_ms: row.get(6)?,
        kind: TimelineEventKind::from_db(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        correlation_id: row.get(8)?,
        request_id: row.get(9)?,
        turn_id: row.get(10)?,
        payload: serde_json::from_str(&payload).map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{AssociateRequestResult, RepositoryError, SessionRepository};
    use crate::{
        AppendEventResult, ModelError, NewSession, NewSessionBrief, NewTimelineEvent,
        RequestTurnAssociation, SessionStatus, TimelineEventKind,
    };
    use rusqlite::params;
    use serde_json::json;
    use std::fs;
    use tempfile::{tempdir, TempDir};
    const WORKSPACE_A: &str = "workspace-a";
    const WORKSPACE_B: &str = "workspace-b";
    const SESSION: &str = "session-a";
    fn key(byte: u8) -> Vec<u8> {
        vec![byte; 32]
    }
    fn repository() -> (TempDir, std::path::PathBuf, SessionRepository) {
        let temp = tempdir().unwrap();
        let path = temp.path().join("sessions.db");
        let repository = SessionRepository::open(&path, &key(0x41)).unwrap();
        (temp, path, repository)
    }

    #[test]
    fn database_key_must_be_exactly_32_bytes() {
        let temp = tempdir().unwrap();
        for length in [0, 1, 16, 31, 33, 64] {
            let path = temp.path().join(format!("invalid-key-{length}.db"));
            assert!(SessionRepository::open(&path, &vec![0x41; length]).is_err());
            assert!(!path.exists());
        }
    }

    #[test]
    fn sqlcipher_key_is_passed_as_bytes_through_the_native_api() {
        let production = include_str!("repository.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();

        assert!(production.contains("ffi::sqlite3_key_v2("));
        assert!(production.contains("ffi::SQLITE_OK"));
        assert!(production.contains("connection.handle()"));
        assert!(!production.contains("PRAGMA key"));
        assert!(!production.contains("cipher_key_hex"));
        assert!(!production.contains("cipher_key_pragma"));
    }

    fn session(workspace_id: &str) -> NewSession {
        NewSession {
            workspace_id: workspace_id.into(),
            session_id: SESSION.into(),
            mode: "interview".into(),
            status: SessionStatus::Active,
            ui_language: Some("en-US".into()),
            input_language: "en-US".into(),
            response_language: "en-US".into(),
            review_language: "en-US".into(),
            started_at_ms: 1_700_000_000_000,
            completed_at_ms: None,
        }
    }
    fn event(event_id: &str, kind: TimelineEventKind) -> NewTimelineEvent {
        NewTimelineEvent {
            workspace_id: WORKSPACE_A.into(),
            session_id: SESSION.into(),
            event_id: event_id.into(),
            source_generation: 1,
            source_sequence: 1,
            timestamp_ms: 1_700_000_000_001,
            kind,
            correlation_id: Some("correlation-a".into()),
            request_id: None,
            turn_id: None,
            payload: json!({"text": "CONFIDENTIAL_MARKER_47"}),
        }
    }
    #[test]
    fn sqlcipher_is_active_and_wrong_key_cannot_open_database() {
        let (_temp, path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        assert!(!repository.cipher_version().unwrap().is_empty());
        drop(repository);
        let bytes = fs::read(&path).unwrap();
        assert_ne!(&bytes[..16], b"SQLite format 3\0");
        assert!(SessionRepository::open(&path, &key(0x42)).is_err());
    }
    #[test]
    fn encrypted_file_does_not_leak_payload_marker() {
        let (_temp, path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let mut transcript = event("event-a", TimelineEventKind::TranscriptFinal);
        transcript.turn_id = Some("turn-a".into());
        repository.append_event(&transcript).unwrap();
        drop(repository);
        for sidecar in [
            path.clone(),
            path.with_extension("db-wal"),
            path.with_extension("db-shm"),
        ] {
            if sidecar.exists() {
                let bytes = fs::read(sidecar).unwrap();
                assert!(!bytes
                    .windows(b"CONFIDENTIAL_MARKER_47".len())
                    .any(|window| window == b"CONFIDENTIAL_MARKER_47"));
            }
        }
    }

    #[test]
    fn repositories_can_open_and_close_sequentially_in_one_process() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("sequential.db");

        {
            let repository = SessionRepository::open(&path, &key(0x41)).unwrap();
            repository.create_session(&session(WORKSPACE_A)).unwrap();
        }

        let reopened = SessionRepository::open(&path, &key(0x41)).unwrap();
        assert_eq!(reopened.list_sessions(WORKSPACE_A, 10).unwrap().len(), 1);
    }

    #[test]
    fn duplicate_event_is_idempotent_but_changed_content_collides() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let mut transcript = event("event-a", TimelineEventKind::TranscriptFinal);
        transcript.turn_id = Some("turn-a".into());
        assert_eq!(
            repository.append_event(&transcript).unwrap(),
            AppendEventResult::Inserted { host_sequence: 1 }
        );
        assert_eq!(
            repository.append_event(&transcript).unwrap(),
            AppendEventResult::Duplicate { host_sequence: 1 }
        );
        transcript.payload = json!({"text": "changed"});
        assert!(matches!(
            repository.append_event(&transcript),
            Err(RepositoryError::EventContentCollision { .. })
        ));
        assert_eq!(
            repository.get_timeline(WORKSPACE_A, SESSION).unwrap().len(),
            1
        );
    }

    #[test]
    fn control_delimiters_cannot_alias_distinct_event_content() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let mut first = event("delimiter-event", TimelineEventKind::Note);
        first.correlation_id = Some("association\u{1f}request".into());
        first.request_id = None;
        repository.append_event(&first).unwrap();

        let mut aliased_by_old_fingerprint = first.clone();
        aliased_by_old_fingerprint.correlation_id = Some("association".into());
        aliased_by_old_fingerprint.request_id = Some("request".into());
        aliased_by_old_fingerprint.turn_id = Some("\u{1f}".into());
        assert!(matches!(
            repository.append_event(&aliased_by_old_fingerprint),
            Err(RepositoryError::EventContentCollision { .. })
        ));
    }

    #[test]
    fn equivalent_json_object_order_is_idempotent() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let mut note = event("json-order-event", TimelineEventKind::Note);
        note.payload = json!({"alpha": 1, "beta": 2});
        repository.append_event(&note).unwrap();
        repository
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE timeline_events SET payload_json = ?1 WHERE event_id = ?2",
                params!["{\"beta\":2,\"alpha\":1}", note.event_id],
            )
            .unwrap();

        assert_eq!(
            repository.append_event(&note).unwrap(),
            AppendEventResult::Duplicate { host_sequence: 1 }
        );
    }

    #[test]
    fn sessions_and_briefs_reject_empty_ownership() {
        let (_temp, _path, repository) = repository();
        let mut invalid_session = session(WORKSPACE_A);
        invalid_session.workspace_id.clear();
        assert!(matches!(
            repository.create_session(&invalid_session),
            Err(RepositoryError::Model(ModelError::MissingWorkspaceId))
        ));

        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let invalid_brief = NewSessionBrief {
            workspace_id: WORKSPACE_A.into(),
            session_id: String::new(),
            brief: json!({"summary": "brief"}),
            updated_at_ms: 1,
        };
        assert!(matches!(
            repository.save_session_brief(&invalid_brief),
            Err(RepositoryError::Model(ModelError::MissingSessionId))
        ));
    }
    #[test]
    fn host_order_remains_monotonic_when_source_sequence_resets() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let mut before_restart = event("event-before", TimelineEventKind::SessionState);
        before_restart.source_generation = 4;
        before_restart.source_sequence = 900;
        let mut after_restart = event("event-after", TimelineEventKind::SessionState);
        after_restart.source_generation = 5;
        after_restart.source_sequence = 0;
        repository.append_event(&before_restart).unwrap();
        repository.append_event(&after_restart).unwrap();
        let timeline = repository.get_timeline(WORKSPACE_A, SESSION).unwrap();
        assert_eq!(
            timeline
                .iter()
                .map(|event| event.host_sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            timeline
                .iter()
                .map(|event| event.source_sequence)
                .collect::<Vec<_>>(),
            [900, 0]
        );
    }
    #[test]
    fn request_to_turn_association_survives_reopen() {
        let (_temp, path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let mut suggestion = event("suggestion-a", TimelineEventKind::SuggestionCompleted);
        suggestion.request_id = Some("request-a".into());
        suggestion.turn_id = Some("turn-a".into());
        repository.append_event(&suggestion).unwrap();
        drop(repository);
        let reopened = SessionRepository::open(&path, &key(0x41)).unwrap();
        let stored = reopened.get_timeline(WORKSPACE_A, SESSION).unwrap();
        assert_eq!(stored[0].request_id.as_deref(), Some("request-a"));
        assert_eq!(stored[0].turn_id.as_deref(), Some("turn-a"));
    }
    #[test]
    fn partials_and_chunks_are_never_persisted() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        for kind in [
            TimelineEventKind::TranscriptPartial,
            TimelineEventKind::SuggestionChunk,
        ] {
            assert!(matches!(
                repository.append_event(&event("transient", kind)),
                Err(RepositoryError::NonDurableEvent(_))
            ));
        }
        assert!(repository
            .get_timeline(WORKSPACE_A, SESSION)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn repository_enforces_workspace_ownership_and_cascades_deletes() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        repository.create_session(&session(WORKSPACE_B)).unwrap();
        repository
            .save_session_brief(&NewSessionBrief {
                workspace_id: WORKSPACE_A.into(),
                session_id: SESSION.into(),
                brief: json!({"summary": "private brief"}),
                updated_at_ms: 1_700_000_000_002,
            })
            .unwrap();
        let note = event("note-a", TimelineEventKind::Note);
        repository.append_event(&note).unwrap();
        assert!(repository
            .get_session(WORKSPACE_B, SESSION)
            .unwrap()
            .is_some());
        assert!(repository
            .get_timeline(WORKSPACE_B, SESSION)
            .unwrap()
            .is_empty());
        assert!(repository.delete_session(WORKSPACE_A, SESSION).unwrap());
        assert!(repository
            .get_session(WORKSPACE_A, SESSION)
            .unwrap()
            .is_none());
        assert!(repository
            .get_timeline(WORKSPACE_A, SESSION)
            .unwrap()
            .is_empty());
        assert!(repository
            .get_session(WORKSPACE_B, SESSION)
            .unwrap()
            .is_some());
    }
    #[test]
    fn restore_returns_latest_incomplete_session_for_workspace() {
        let (_temp, _path, repository) = repository();
        let mut older = session(WORKSPACE_A);
        older.session_id = "older".into();
        let mut newer = session(WORKSPACE_A);
        newer.session_id = "newer".into();
        newer.started_at_ms += 10;
        repository.create_session(&older).unwrap();
        repository.create_session(&newer).unwrap();
        repository
            .complete_session(
                WORKSPACE_A,
                "newer",
                SessionStatus::Completed,
                1_700_000_000_020,
            )
            .unwrap();
        assert_eq!(
            repository
                .restore_active_session(WORKSPACE_A)
                .unwrap()
                .unwrap()
                .session_id,
            "older"
        );
        assert!(repository
            .restore_active_session(WORKSPACE_B)
            .unwrap()
            .is_none());
    }

    #[test]
    fn list_sessions_and_timeline_materialize_rows_before_connection_unlock() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let note = event("list-and-timeline", TimelineEventKind::Note);
        repository.append_event(&note).unwrap();

        assert_eq!(repository.list_sessions(WORKSPACE_A, 10).unwrap().len(), 1);
        assert_eq!(
            repository.get_timeline(WORKSPACE_A, SESSION).unwrap().len(),
            1
        );
    }

    #[test]
    fn sessions_materialize_json_briefs_and_restore_only_active_status() {
        let (_temp, _path, repository) = repository();
        let mut interrupted = session(WORKSPACE_A);
        interrupted.session_id = "interrupted".into();
        interrupted.status = SessionStatus::Interrupted;
        interrupted.completed_at_ms = Some(1_700_000_000_020);
        repository.create_session(&interrupted).unwrap();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let brief = json!({"role": "staff engineer", "topics": ["Rust", "SQLite"]});
        repository
            .save_session_brief(&NewSessionBrief {
                workspace_id: WORKSPACE_A.into(),
                session_id: SESSION.into(),
                brief: brief.clone(),
                updated_at_ms: 1_700_000_000_030,
            })
            .unwrap();

        let stored = repository
            .get_session(WORKSPACE_A, SESSION)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, SessionStatus::Active);
        assert_eq!(stored.brief.unwrap().brief, brief);
        assert_eq!(
            repository
                .restore_active_session(WORKSPACE_A)
                .unwrap()
                .unwrap()
                .session_id,
            SESSION
        );
    }

    #[test]
    fn complete_session_accepts_interrupted_status() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();

        assert!(repository
            .complete_session(
                WORKSPACE_A,
                SESSION,
                SessionStatus::Interrupted,
                1_700_000_000_050,
            )
            .unwrap());
        let stored = repository
            .get_session(WORKSPACE_A, SESSION)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, SessionStatus::Interrupted);
        assert_eq!(stored.completed_at_ms, Some(1_700_000_000_050));
    }

    #[test]
    fn list_sessions_rejects_out_of_range_limits() {
        let (_temp, _path, repository) = repository();
        assert!(matches!(
            repository.list_sessions(WORKSPACE_A, 0),
            Err(RepositoryError::InvalidSessionLimit { limit: 0 })
        ));
        assert!(matches!(
            repository.list_sessions(WORKSPACE_A, 101),
            Err(RepositoryError::InvalidSessionLimit { limit: 101 })
        ));
    }

    #[test]
    fn request_turn_association_is_idempotent_and_rejects_conflicts() {
        let (_temp, _path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let association = RequestTurnAssociation {
            workspace_id: WORKSPACE_A.into(),
            session_id: SESSION.into(),
            request_id: "request-a".into(),
            turn_id: "turn-a".into(),
            created_at_ms: 1_700_000_000_040,
        };
        assert_eq!(
            repository
                .associate_request_with_turn(&association)
                .unwrap(),
            AssociateRequestResult::Inserted
        );
        assert_eq!(
            repository
                .associate_request_with_turn(&association)
                .unwrap(),
            AssociateRequestResult::Duplicate
        );
        let mut conflicting = association.clone();
        conflicting.turn_id = "turn-b".into();
        assert!(matches!(
            repository.associate_request_with_turn(&conflicting),
            Err(RepositoryError::RequestTurnAssociationConflict { .. })
        ));
        assert_eq!(
            repository
                .resolve_request_turn(WORKSPACE_A, SESSION, "request-a")
                .unwrap()
                .as_deref(),
            Some("turn-a")
        );
    }

    #[test]
    fn request_turn_associations_persist_and_cascade_with_owned_session() {
        let (_temp, path, repository) = repository();
        repository.create_session(&session(WORKSPACE_A)).unwrap();
        let association = RequestTurnAssociation {
            workspace_id: WORKSPACE_A.into(),
            session_id: SESSION.into(),
            request_id: "request-a".into(),
            turn_id: "turn-a".into(),
            created_at_ms: 1_700_000_000_040,
        };
        repository
            .associate_request_with_turn(&association)
            .unwrap();
        assert!(matches!(
            repository.associate_request_with_turn(&RequestTurnAssociation {
                workspace_id: WORKSPACE_B.into(),
                ..association.clone()
            }),
            Err(RepositoryError::NotFound)
        ));
        drop(repository);

        let reopened = SessionRepository::open(&path, &key(0x41)).unwrap();
        assert_eq!(
            reopened
                .get_request_turn_associations(WORKSPACE_A, SESSION)
                .unwrap(),
            vec![association]
        );
        assert!(reopened.delete_session(WORKSPACE_A, SESSION).unwrap());
        assert!(reopened
            .get_request_turn_associations(WORKSPACE_A, SESSION)
            .unwrap()
            .is_empty());
    }
}
