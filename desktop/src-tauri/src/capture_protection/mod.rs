use std::sync::{Arc, RwLock};

use serde::Serialize;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::sidecar::SidecarError;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use macos::platform_target;
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use unsupported::platform_target;
#[cfg(windows)]
pub(crate) use windows::platform_target;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureProtectionState {
    Applying,
    Protected,
    Unavailable,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureProtectionStatus {
    pub state: CaptureProtectionState,
    pub code: Option<&'static str>,
    pub message: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureProtectionFailure {
    ApplyFailed,
    VerificationFailed,
    Unsupported,
}

pub trait CaptureProtectionTarget: Send + Sync {
    fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure>;
}

pub struct CaptureProtectionController {
    target: Arc<dyn CaptureProtectionTarget>,
    state: RwLock<CaptureProtectionRecord>,
    operation: Arc<AsyncMutex<()>>,
}

struct CaptureProtectionRecord {
    generation: u64,
    status: CaptureProtectionStatus,
}

pub struct CaptureProtectionLease {
    _operation: OwnedMutexGuard<()>,
}

impl CaptureProtectionController {
    pub fn with_target(target: Arc<dyn CaptureProtectionTarget>) -> Self {
        Self {
            target,
            state: RwLock::new(CaptureProtectionRecord {
                generation: 0,
                status: CaptureProtectionStatus::applying(),
            }),
            operation: Arc::new(AsyncMutex::new(())),
        }
    }

    pub fn status(&self) -> CaptureProtectionStatus {
        self.state
            .read()
            .expect("capture status lock poisoned")
            .status
            .clone()
    }

    pub fn reapply_and_verify(&self) -> CaptureProtectionStatus {
        let _operation = self.operation.clone().blocking_lock_owned();
        self.reapply_and_verify_while_serialized()
    }

    pub async fn reapply_and_verify_async(&self) -> CaptureProtectionStatus {
        let _operation = self.operation.lock().await;
        self.reapply_and_verify_while_serialized()
    }

    pub async fn acquire_protected_lease(&self) -> Result<CaptureProtectionLease, SidecarError> {
        let operation = self.operation.clone().lock_owned().await;
        self.reapply_and_verify_while_serialized();
        self.require_protected()?;

        Ok(CaptureProtectionLease {
            _operation: operation,
        })
    }

    fn reapply_and_verify_while_serialized(&self) -> CaptureProtectionStatus {
        let generation = {
            let mut state = self.state.write().expect("capture status lock poisoned");
            state.generation += 1;
            state.status = CaptureProtectionStatus::applying();
            state.generation
        };

        let status = match self.target.apply_and_verify() {
            Ok(()) => CaptureProtectionStatus::protected(),
            Err(failure) => CaptureProtectionStatus::from_failure(failure),
        };
        let mut state = self.state.write().expect("capture status lock poisoned");
        if state.generation == generation {
            state.status = status.clone();
        }
        status
    }

    pub fn require_protected(&self) -> Result<(), SidecarError> {
        if self.status().state == CaptureProtectionState::Protected {
            Ok(())
        } else {
            Err(SidecarError::new(
                "capture_protection_required",
                "Live mode is unavailable because screen capture protection could not be confirmed.",
            ))
        }
    }

    #[cfg(test)]
    fn with_status(status: CaptureProtectionStatus) -> Self {
        Self {
            target: Arc::new(UnsupportedTarget),
            state: RwLock::new(CaptureProtectionRecord {
                generation: 0,
                status,
            }),
            operation: Arc::new(AsyncMutex::new(())),
        }
    }

    #[cfg(test)]
    fn with_status_for_test(state: CaptureProtectionState) -> Self {
        Self::with_status(CaptureProtectionStatus {
            state,
            code: None,
            message: None,
        })
    }
}

impl CaptureProtectionStatus {
    fn applying() -> Self {
        Self {
            state: CaptureProtectionState::Applying,
            code: Some("capture_protection_applying"),
            message: Some("Screen capture protection is being applied."),
        }
    }

    fn protected() -> Self {
        Self {
            state: CaptureProtectionState::Protected,
            code: None,
            message: None,
        }
    }

    fn from_failure(failure: CaptureProtectionFailure) -> Self {
        match failure {
            CaptureProtectionFailure::Unsupported => Self {
                state: CaptureProtectionState::Unsupported,
                code: Some("capture_protection_unsupported"),
                message: Some("Screen capture protection is unsupported on this platform."),
            },
            CaptureProtectionFailure::ApplyFailed
            | CaptureProtectionFailure::VerificationFailed => Self {
                state: CaptureProtectionState::Unavailable,
                code: Some("capture_protection_unavailable"),
                message: Some("Screen capture protection could not be confirmed."),
            },
        }
    }
}

#[cfg(test)]
struct UnsupportedTarget;

#[cfg(test)]
impl CaptureProtectionTarget for UnsupportedTarget {
    fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
        Err(CaptureProtectionFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;

    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_requires_exact_exclude_from_capture_affinity() {
        assert!(crate::capture_protection::windows::is_excluded_affinity(17));
        for affinity in [0, 1, 16, 18, u32::MAX] {
            assert!(!crate::capture_protection::windows::is_excluded_affinity(
                affinity
            ));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_requires_exact_none_sharing_type() {
        use objc2_app_kit::NSWindowSharingType;

        assert!(crate::capture_protection::macos::is_excluded_sharing_type(
            NSWindowSharingType::None
        ));
        assert!(!crate::capture_protection::macos::is_excluded_sharing_type(
            NSWindowSharingType::ReadOnly
        ));
        assert!(!crate::capture_protection::macos::is_excluded_sharing_type(
            NSWindowSharingType(2)
        ));
        assert!(!crate::capture_protection::macos::is_excluded_sharing_type(
            NSWindowSharingType(usize::MAX)
        ));
    }

    struct FakeTarget {
        outcomes: Mutex<VecDeque<Result<(), CaptureProtectionFailure>>>,
    }

    impl FakeTarget {
        fn new(outcomes: impl IntoIterator<Item = Result<(), CaptureProtectionFailure>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
            }
        }
    }

    impl CaptureProtectionTarget for FakeTarget {
        fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("fake target outcome queue must not be empty")
        }
    }

    struct BlockingTarget {
        state: Mutex<BlockingTargetState>,
        wake: Condvar,
    }

    #[derive(Default)]
    struct BlockingTargetState {
        started: usize,
        released: [bool; 2],
    }

    impl BlockingTarget {
        fn wait_until_started(&self, count: usize) {
            let mut state = self.state.lock().unwrap();
            while state.started < count {
                state = self.wake.wait(state).unwrap();
            }
        }

        fn release(&self, call: usize) {
            let mut state = self.state.lock().unwrap();
            state.released[call - 1] = true;
            self.wake.notify_all();
        }
    }

    impl CaptureProtectionTarget for BlockingTarget {
        fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
            let mut state = self.state.lock().unwrap();
            let call = state.started;
            state.started += 1;
            self.wake.notify_all();
            while !state.released[call] {
                state = self.wake.wait(state).unwrap();
            }
            Ok(())
        }
    }

    #[test]
    fn only_a_confirmed_target_result_marks_the_window_protected() {
        let target = Arc::new(FakeTarget::new([
            Err(CaptureProtectionFailure::ApplyFailed),
            Ok(()),
        ]));
        let controller = CaptureProtectionController::with_target(target);

        assert_eq!(
            controller.reapply_and_verify().state,
            CaptureProtectionState::Unavailable
        );
        assert_eq!(
            controller.reapply_and_verify().state,
            CaptureProtectionState::Protected
        );
    }

    #[test]
    fn newer_reapply_cannot_be_bypassed_by_stale_protected_result() {
        let target = Arc::new(BlockingTarget {
            state: Mutex::new(BlockingTargetState::default()),
            wake: Condvar::new(),
        });
        let controller = Arc::new(CaptureProtectionController::with_target(target.clone()));

        let first_controller = controller.clone();
        let first = thread::spawn(move || first_controller.reapply_and_verify());
        target.wait_until_started(1);

        let second_controller = controller.clone();
        let second = thread::spawn(move || second_controller.reapply_and_verify());

        target.release(1);
        let first_status = first.join().unwrap();
        target.wait_until_started(2);
        let protection_error = controller
            .require_protected()
            .err()
            .map(|error| error.code());

        target.release(2);
        let second_status = second.join().unwrap();

        assert_eq!(first_status.state, CaptureProtectionState::Protected);
        assert_eq!(second_status.state, CaptureProtectionState::Protected);
        assert_eq!(protection_error, Some("capture_protection_required"));
    }

    #[test]
    fn every_non_protected_state_fails_closed() {
        for state in [
            CaptureProtectionState::Applying,
            CaptureProtectionState::Unavailable,
            CaptureProtectionState::Unsupported,
        ] {
            let controller = CaptureProtectionController::with_status_for_test(state);
            assert_eq!(
                controller.require_protected().unwrap_err().code(),
                "capture_protection_required"
            );
        }
    }
}
