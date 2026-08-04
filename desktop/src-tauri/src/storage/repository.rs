use std::{path::Path, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::{
    migrations::{migrate, MigrationError},
    AppendEventResult, ModelError, NewSession, NewSessionBrief, NewTimelineEvent, StoredSession,
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
        if key.is_empty() {
            return Err(RepositoryError::CipherUnavailable);
        }
        let mut connection = Connection::open(path)?;
        apply_cipher_key(&mut connection, key)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA cipher_memory_security = ON; PRAGMA journal_mode = DELETE;")?;
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
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        connection.execute("INSERT INTO sessions (workspace_id, session_id, title, language, started_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)", params![session.workspace_id, session.session_id, session.title, session.language, session.started_at_ms])?;
        Ok(())
    }

    pub fn save_session_brief(&self, brief: &NewSessionBrief) -> Result<(), RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let updated = connection.execute("INSERT INTO session_briefs (workspace_id, session_id, summary, updated_at_ms) SELECT workspace_id, session_id, ?3, ?4 FROM sessions WHERE workspace_id = ?1 AND session_id = ?2 ON CONFLICT(workspace_id, session_id) DO UPDATE SET summary = excluded.summary, updated_at_ms = excluded.updated_at_ms", params![brief.workspace_id, brief.session_id, brief.summary, brief.updated_at_ms])?;
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
            if existing == event_fingerprint(event, &payload_json) {
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
        completed_at_ms: i64,
    ) -> Result<bool, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        Ok(connection.execute(
            "UPDATE sessions SET completed_at_ms = ?3 WHERE workspace_id = ?1 AND session_id = ?2",
            params![workspace_id, session_id, completed_at_ms],
        )? > 0)
    }

    pub fn list_sessions(&self, workspace_id: &str) -> Result<Vec<StoredSession>, RepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| RepositoryError::ConnectionUnavailable)?;
        let mut statement = connection.prepare("SELECT workspace_id, session_id, title, language, started_at_ms, completed_at_ms FROM sessions WHERE workspace_id = ?1 ORDER BY started_at_ms DESC")?;
        let sessions = statement
            .query_map([workspace_id], session_from_row)?
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
        connection.query_row("SELECT workspace_id, session_id, title, language, started_at_ms, completed_at_ms FROM sessions WHERE workspace_id = ?1 AND session_id = ?2", params![workspace_id, session_id], session_from_row).optional().map_err(Into::into)
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
        connection.query_row("SELECT workspace_id, session_id, title, language, started_at_ms, completed_at_ms FROM sessions WHERE workspace_id = ?1 AND completed_at_ms IS NULL ORDER BY started_at_ms DESC LIMIT 1", [workspace_id], session_from_row).optional().map_err(Into::into)
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
}

fn apply_cipher_key(connection: &mut Connection, key: &[u8]) -> Result<(), RepositoryError> {
    let hex_key: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    connection.execute_batch(&format!("PRAGMA key = \"x'{hex_key}'\";"))?;
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

fn event_fingerprint(event: &NewTimelineEvent, payload_json: &str) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        event.workspace_id,
        event.session_id,
        event.source_generation,
        event.source_sequence,
        event.timestamp_ms,
        event.kind.as_db(),
        event.correlation_id.as_deref().unwrap_or_default(),
        event.request_id.as_deref().unwrap_or_default(),
        event.turn_id.as_deref().unwrap_or_default()
    ) + payload_json
}

fn existing_event(
    transaction: &Transaction<'_>,
    event_id: &str,
) -> Result<Option<(i64, String)>, RepositoryError> {
    transaction.query_row("SELECT host_sequence, workspace_id, session_id, source_generation, source_sequence, timestamp_ms, kind, correlation_id, request_id, turn_id, payload_json FROM timeline_events WHERE event_id = ?1", [event_id], |row| {
        let fingerprint = format!("{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}", row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?, row.get::<_, String>(6)?, row.get::<_, Option<String>>(7)?.as_deref().unwrap_or_default(), row.get::<_, Option<String>>(8)?.as_deref().unwrap_or_default(), row.get::<_, Option<String>>(9)?.as_deref().unwrap_or_default()) + &row.get::<_, String>(10)?;
        Ok((row.get(0)?, fingerprint))
    }).optional().map_err(Into::into)
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSession> {
    Ok(StoredSession {
        workspace_id: row.get(0)?,
        session_id: row.get(1)?,
        title: row.get(2)?,
        language: row.get(3)?,
        started_at_ms: row.get(4)?,
        completed_at_ms: row.get(5)?,
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
    use super::{RepositoryError, SessionRepository};
    use crate::{
        AppendEventResult, NewSession, NewSessionBrief, NewTimelineEvent, TimelineEventKind,
    };
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
    fn session(workspace_id: &str) -> NewSession {
        NewSession {
            workspace_id: workspace_id.into(),
            session_id: SESSION.into(),
            title: Some("Interview".into()),
            language: "en-US".into(),
            started_at_ms: 1_700_000_000_000,
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
                summary: "private brief".into(),
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
            .complete_session(WORKSPACE_A, "newer", 1_700_000_000_020)
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

        assert_eq!(repository.list_sessions(WORKSPACE_A).unwrap().len(), 1);
        assert_eq!(
            repository.get_timeline(WORKSPACE_A, SESSION).unwrap().len(),
            1
        );
    }
}
