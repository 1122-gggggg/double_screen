//! Linux host session backend.
//!
//! Per-user Weston is spawned as that UID with `-Bpipewire --renderer=gl`, an
//! independent Wayland socket, and a compositor-local virtual seat. The live
//! path requires PipeWire and NVENC; it fails closed instead of reporting a
//! headless session with no transport. It never takes seat0 DRM master.

mod backend;
mod detect;
mod diagnostics;
mod input;
mod media;
#[cfg(target_os = "linux")]
mod spawn;

pub use backend::{wayland_display_for, LinuxSessionBackend, LinuxSessionRuntime};
pub use detect::{detect_host_features, detect_linux_capabilities, HostFeatures};
pub use diagnostics::{diagnostics_gpu, GpuDiagnostics};
pub use input::WestonInputBackend;
pub use media::{AnnexBAccessUnitReader, PipeWireNvencCapture};

pub use splitdesk_core::{
    CaptureKind, CreateSessionRequest, EncoderKind, Error, HostOs, Resolution, SessionId,
    SessionStatus, UserName,
};
pub use splitdesk_session::{SessionBackend, SessionRecord};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{isolation_holds, session_numeric};

    #[test]
    fn wayland_display_unique_per_session() {
        let a = SessionId::new(1);
        let b = SessionId::new(2);
        let da = wayland_display_for(&a);
        let db = wayland_display_for(&b);
        assert_eq!(da, "splitdesk-1");
        assert_eq!(db, "splitdesk-2");
        assert_ne!(da, db);
        assert_eq!(session_numeric(&SessionId::new(12)), 12);
        assert_eq!(wayland_display_for(&SessionId::new(12)), "splitdesk-12");
    }

    #[test]
    fn isolation_of_user_fields() {
        let alice = SessionRecord {
            id: SessionId::new(1),
            user: UserName::new("alice"),
            os: HostOs::Linux,
            resolution: Resolution::default(),
            fps: 60,
            status: SessionStatus::Running,
            wayland_display: Some(wayland_display_for(&SessionId::new(1))),
            windows_session_id: None,
            encoder: EncoderKind::Unavailable,
            capture: CaptureKind::Unavailable,
        };
        let bob = SessionRecord {
            id: SessionId::new(2),
            user: UserName::new("bob"),
            os: HostOs::Linux,
            resolution: Resolution::default(),
            fps: 60,
            status: SessionStatus::Running,
            wayland_display: Some(wayland_display_for(&SessionId::new(2))),
            windows_session_id: None,
            encoder: EncoderKind::Unavailable,
            capture: CaptureKind::Unavailable,
        };
        assert!(isolation_holds(&alice, &bob));
        assert_ne!(alice.wayland_display, bob.wayland_display);
        assert!(alice.windows_session_id.is_none());
        assert!(bob.windows_session_id.is_none());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_create_returns_unavailable() {
        let backend = LinuxSessionBackend::new();
        let req = CreateSessionRequest::new("alice");
        match backend.create_session(req) {
            Err(Error::BackendUnavailable { detail }) => {
                assert_eq!(detail, "linux host backend not available on this OS");
            }
            other => panic!("unexpected result: {other:?}"),
        }
        assert!(backend.list_sessions().unwrap().is_empty());
        assert!(matches!(
            backend.info(&SessionId::new(1)),
            Err(Error::SessionNotFound)
        ));
    }

    #[test]
    fn gpu_diagnostics_never_panic() {
        let _ = diagnostics_gpu();
        let features = detect_host_features();
        let caps = detect_linux_capabilities();
        assert_eq!(caps.session_support, features.session_support);
        assert_eq!(
            caps.session_support,
            splitdesk_core::SessionSupport::LinuxMultiUser
        );
        assert!(!caps.dxgi);
        assert!(!caps.rds);
    }
}
