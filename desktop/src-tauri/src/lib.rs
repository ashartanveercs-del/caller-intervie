// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

pub mod capture_protection;
pub mod commands;
pub mod protocol;
pub mod sidecar;
pub mod state;
pub mod storage;

const EXIT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_IDLE: u8 = 0;
const SHUTDOWN_RUNNING: u8 = 1;
const SHUTDOWN_COMPLETE: u8 = 2;

#[derive(Clone, Default)]
struct ExitShutdownCoordinator {
    phase: Arc<AtomicU8>,
}

enum ExitShutdownAction {
    Start(ExitShutdownPermit),
    Wait,
    Allow,
}

struct ExitShutdownPermit {
    coordinator: ExitShutdownCoordinator,
    complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownOutcome {
    Completed,
    Failed,
    TimedOut,
}

impl ExitShutdownCoordinator {
    fn on_exit_requested(&self, smoke_mode: bool) -> ExitShutdownAction {
        if smoke_mode {
            return ExitShutdownAction::Allow;
        }

        match self.phase.compare_exchange(
            SHUTDOWN_IDLE,
            SHUTDOWN_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => ExitShutdownAction::Start(ExitShutdownPermit {
                coordinator: self.clone(),
                complete: false,
            }),
            Err(SHUTDOWN_RUNNING) => ExitShutdownAction::Wait,
            Err(_) => ExitShutdownAction::Allow,
        }
    }
}

impl ExitShutdownPermit {
    async fn run<F, E>(mut self, timeout: Duration, shutdown: F) -> ShutdownOutcome
    where
        F: Future<Output = Result<(), E>>,
    {
        let outcome = match tokio::time::timeout(timeout, shutdown).await {
            Ok(Ok(())) => ShutdownOutcome::Completed,
            Ok(Err(_)) => ShutdownOutcome::Failed,
            Err(_) => ShutdownOutcome::TimedOut,
        };
        self.mark_complete();
        outcome
    }

    fn mark_complete(&mut self) {
        self.coordinator
            .phase
            .store(SHUTDOWN_COMPLETE, Ordering::Release);
        self.complete = true;
    }
}

impl Drop for ExitShutdownPermit {
    fn drop(&mut self) {
        if !self.complete {
            self.mark_complete();
        }
    }
}

impl ShutdownOutcome {
    fn failure_log(self) -> Option<&'static str> {
        match self {
            Self::Completed => None,
            Self::Failed => Some("sidecar shutdown failed: code=sidecar_shutdown_failed"),
            Self::TimedOut => Some("sidecar shutdown failed: code=sidecar_shutdown_timeout"),
        }
    }
}

fn sidecar_smoke_requested() -> bool {
    std::env::args().any(|argument| argument == "--sidecar-smoke")
}

async fn wait_for_smoke_ready(sidecar: &sidecar::SidecarSupervisor) -> Result<(), String> {
    tokio::time::timeout(
        sidecar::PRODUCTION_HANDSHAKE_TIMEOUT + Duration::from_secs(5),
        async {
            loop {
                match sidecar.status().await.state {
                    sidecar::SidecarState::Ready => return Ok(()),
                    sidecar::SidecarState::Failed => {
                        return Err("sidecar entered failed state during handshake".into())
                    }
                    _ => tokio::time::sleep(Duration::from_millis(25)).await,
                }
            }
        },
    )
    .await
    .map_err(|_| "sidecar did not emit correlated sidecar.ready within 50 seconds".to_owned())?
}

async fn finish_smoke(
    sidecar: &sidecar::SidecarSupervisor,
    readiness: Result<(), String>,
) -> Result<(), String> {
    let cleanup = sidecar.shutdown().await.map_err(|error| error.to_string());
    match (readiness, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (_, Err(error)) => Err(format!("sidecar smoke cleanup failed: {error}")),
        (Err(error), Ok(())) => Err(error),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let smoke_mode = sidecar_smoke_requested();
    let exit_shutdown = ExitShutdownCoordinator::default();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let state = state::AppState::new(app.handle().clone())?;
            let sidecar = state.sidecar.clone();
            let app_handle = app.handle().clone();
            app.manage(state);
            tauri::async_runtime::spawn(async move {
                let start = sidecar.start().await;
                if !smoke_mode {
                    return;
                }
                let readiness = match start {
                    Ok(_) => wait_for_smoke_ready(&sidecar).await,
                    Err(error) => Err(error.to_string()),
                };
                let result = finish_smoke(&sidecar, readiness).await;
                if let Err(error) = result {
                    eprintln!("sidecar smoke failed: {error}");
                    app_handle.exit(1);
                } else {
                    app_handle.exit(0);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::sidecar_status,
            commands::storage_health,
            commands::send_sidecar_command,
            commands::restart_sidecar,
            commands::create_session,
            commands::save_session_brief,
            commands::complete_session,
            commands::list_sessions,
            commands::get_session,
            commands::get_timeline,
            commands::restore_active_session,
            commands::delete_session,
            commands::associate_request_with_turn,
            commands::get_request_turn_associations
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(move |app_handle, event| {
        let tauri::RunEvent::ExitRequested { code, api, .. } = event else {
            return;
        };

        // Tauri restart requests cannot be prevented, so only deferrable exits use this path.
        let bypass_shutdown = smoke_mode || code == Some(tauri::RESTART_EXIT_CODE);
        match exit_shutdown.on_exit_requested(bypass_shutdown) {
            ExitShutdownAction::Start(permit) => {
                api.prevent_exit();
                let sidecar = app_handle.state::<state::AppState>().sidecar.clone();
                let app_handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let outcome = permit.run(EXIT_SHUTDOWN_TIMEOUT, sidecar.shutdown()).await;
                    if let Some(diagnostic) = outcome.failure_log() {
                        eprintln!("{diagnostic}");
                    }
                    app_handle.exit(code.unwrap_or_default());
                });
            }
            ExitShutdownAction::Wait => api.prevent_exit(),
            ExitShutdownAction::Allow => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;

    use crate::sidecar::{SidecarError, SidecarPort, SidecarState, SidecarSupervisor};

    use super::{ExitShutdownAction, ExitShutdownCoordinator, ShutdownOutcome};

    #[test]
    fn persistent_session_commands_are_registered_with_tauri() {
        let source = include_str!("lib.rs");
        for command in [
            "commands::storage_health",
            "commands::create_session",
            "commands::save_session_brief",
            "commands::complete_session",
            "commands::list_sessions",
            "commands::get_session",
            "commands::get_timeline",
            "commands::restore_active_session",
            "commands::delete_session",
            "commands::associate_request_with_turn",
            "commands::get_request_turn_associations",
        ] {
            assert!(source.contains(command), "missing Tauri command: {command}");
        }
    }

    #[tokio::test]
    async fn smoke_cleanup_failure_is_reported_as_a_failure() {
        struct FailingCleanupPort;

        #[async_trait]
        impl SidecarPort for FailingCleanupPort {
            async fn write(&self, _bytes: Vec<u8>) -> Result<(), SidecarError> {
                Ok(())
            }

            async fn kill(&self) -> Result<(), SidecarError> {
                Err(SidecarError::new(
                    "sidecar_kill_failed",
                    "DEEPSEEK_API_KEY=cleanup-secret",
                ))
            }
        }

        let sidecar = SidecarSupervisor::with_port(Arc::new(FailingCleanupPort));
        let error = super::finish_smoke(&sidecar, Ok(())).await.unwrap_err();

        assert!(error.contains("cleanup"));
        assert!(!error.contains("cleanup-secret"));
        assert_eq!(sidecar.status().await.state, SidecarState::Failed);
    }

    #[tokio::test]
    async fn normal_exit_runs_shutdown_once_before_allowing_exit() {
        let coordinator = ExitShutdownCoordinator::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let ExitShutdownAction::Start(permit) = coordinator.on_exit_requested(false) else {
            panic!("first normal exit must start shutdown");
        };
        let calls_for_shutdown = calls.clone();

        let outcome = permit
            .run(Duration::from_secs(1), async move {
                calls_for_shutdown.fetch_add(1, Ordering::SeqCst);
                Ok::<(), ()>(())
            })
            .await;

        assert_eq!(outcome, ShutdownOutcome::Completed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            coordinator.on_exit_requested(false),
            ExitShutdownAction::Allow
        ));
    }

    #[tokio::test]
    async fn duplicate_exit_events_wait_for_the_original_shutdown() {
        let coordinator = ExitShutdownCoordinator::default();
        let ExitShutdownAction::Start(permit) = coordinator.on_exit_requested(false) else {
            panic!("first normal exit must start shutdown");
        };

        assert!(matches!(
            coordinator.on_exit_requested(false),
            ExitShutdownAction::Wait
        ));

        assert_eq!(
            permit
                .run(Duration::from_secs(1), async { Ok::<(), ()>(()) })
                .await,
            ShutdownOutcome::Completed
        );
        assert!(matches!(
            coordinator.on_exit_requested(false),
            ExitShutdownAction::Allow
        ));
    }

    #[tokio::test]
    async fn shutdown_timeout_is_bounded_and_reports_only_a_stable_code() {
        let coordinator = ExitShutdownCoordinator::default();
        let ExitShutdownAction::Start(permit) = coordinator.on_exit_requested(false) else {
            panic!("first normal exit must start shutdown");
        };

        let outcome = permit
            .run(Duration::from_millis(10), pending::<Result<(), String>>())
            .await;

        assert_eq!(outcome, ShutdownOutcome::TimedOut);
        assert_eq!(
            outcome.failure_log(),
            Some("sidecar shutdown failed: code=sidecar_shutdown_timeout")
        );
        assert!(matches!(
            coordinator.on_exit_requested(false),
            ExitShutdownAction::Allow
        ));
    }

    #[tokio::test]
    async fn shutdown_failure_discards_the_underlying_error() {
        let coordinator = ExitShutdownCoordinator::default();
        let ExitShutdownAction::Start(permit) = coordinator.on_exit_requested(false) else {
            panic!("first normal exit must start shutdown");
        };

        let outcome = permit
            .run(Duration::from_secs(1), async {
                Err::<(), _>("DEEPSEEK_API_KEY=must-not-leak")
            })
            .await;

        assert_eq!(outcome, ShutdownOutcome::Failed);
        let diagnostic = outcome.failure_log().unwrap();
        assert_eq!(
            diagnostic,
            "sidecar shutdown failed: code=sidecar_shutdown_failed"
        );
        assert!(!diagnostic.contains("must-not-leak"));
    }

    #[test]
    fn smoke_exit_bypasses_the_normal_shutdown_coordinator() {
        let coordinator = ExitShutdownCoordinator::default();

        assert!(matches!(
            coordinator.on_exit_requested(true),
            ExitShutdownAction::Allow
        ));
        assert!(matches!(
            coordinator.on_exit_requested(false),
            ExitShutdownAction::Start(_)
        ));
    }
}
