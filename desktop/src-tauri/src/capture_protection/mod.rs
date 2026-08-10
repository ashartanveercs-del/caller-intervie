use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::sidecar::SidecarError;

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
    status: RwLock<CaptureProtectionStatus>,
}

impl CaptureProtectionController {
    pub fn with_target(target: Arc<dyn CaptureProtectionTarget>) -> Self {
        Self {
            target,
            status: RwLock::new(CaptureProtectionStatus::applying()),
        }
    }

    pub fn status(&self) -> CaptureProtectionStatus {
        self.status
            .read()
            .expect("capture status lock poisoned")
            .clone()
    }

    pub fn reapply_and_verify(&self) -> CaptureProtectionStatus {
        *self.status.write().expect("capture status lock poisoned") =
            CaptureProtectionStatus::applying();

        let status = match self.target.apply_and_verify() {
            Ok(()) => CaptureProtectionStatus::protected(),
            Err(failure) => CaptureProtectionStatus::from_failure(failure),
        };
        *self.status.write().expect("capture status lock poisoned") = status.clone();
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
            status: RwLock::new(status),
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
    use std::sync::{Arc, Mutex};

    use super::*;

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
