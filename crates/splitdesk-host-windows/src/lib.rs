//! Windows host session backend.
//!
//! Windows 10/11 client SKUs support a single interactive session. A second
//! concurrent interactive session returns [`Error::MultiUserNotSupportedByHostOs`].
//! This crate never patches `termsrv.dll` and never uses RDP Wrapper.
//!
//! Live DXGI Desktop Duplication and WTS APIs are compiled only with
//! `cfg(windows)`. Non-Windows builds still compile and return
//! [`Error::BackendUnavailable`] for live spawn.
//!
//! Capture must run in the target interactive WTS session, never Session 0.
//! Inbox `mfh264enc.dll` is CPU YUV and is not advertised as a GPU path.

mod backend;
mod detect;
mod diagnostics;
mod dxgi;
mod input;

#[cfg(windows)]
mod wts;

pub use backend::WindowsSessionBackend;
pub use detect::{default_windows_client_capabilities, detect_windows_capabilities};
pub use diagnostics::diagnostics_gpu;
pub use dxgi::{
    D3D11TextureHandle, DxgiDuplicationCapture, DxgiFormat, DXGI_VENDOR_AMD, DXGI_VENDOR_INTEL,
    DXGI_VENDOR_NVIDIA,
};
pub use input::WindowsInputBackend;

/// Windows Session 0 is the isolated non-interactive service session.
/// DXGI Desktop Duplication and interactive capture must not run there.
pub const WINDOWS_SESSION_0: u32 = 0;

#[cfg(windows)]
pub use wts::{
    active_console_session_id, create_process_as_user, current_process_session_id,
    enumerate_sessions_ex, query_user_token, WtsSession, WtsSessionState,
};

#[cfg(test)]
mod tests {
    use super::*;
    use splitdesk_core::{
        CaptureKind, CreateSessionRequest, Error, HostOs, Resolution, SessionSupport,
    };
    use splitdesk_session::SessionBackend;

    fn request(user: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            user: user.into(),
            resolution: Resolution::default(),
            fps: 60,
            memory_limit: None,
            cpu_affinity: None,
        }
    }

    #[test]
    fn win10_default_capabilities_multi_user_false() {
        let caps = default_windows_client_capabilities();
        assert_eq!(caps.host_os, HostOs::Windows);
        assert!(!caps.multi_user);
        assert!(!caps.rds);
        assert_eq!(
            caps.session_support,
            SessionSupport::WindowsSingleInteractive
        );
        assert_eq!(caps.capture, CaptureKind::Dxgi);
        assert!(caps.dxgi);
        assert!(!caps.pipewire);
        assert!(!caps.wayland);
        assert_ne!(caps.encoder, splitdesk_core::EncoderKind::SoftwareFallback);
    }

    #[test]
    fn second_session_error_discriminant() {
        let backend = WindowsSessionBackend::occupied_single_interactive();
        let err = backend.create_session(request("alice")).unwrap_err();
        assert!(matches!(err, Error::MultiUserNotSupportedByHostOs));
    }

    #[test]
    fn non_windows_live_spawn_is_backend_unavailable() {
        if cfg!(windows) {
            return;
        }
        let backend = WindowsSessionBackend::new();
        let err = backend.create_session(request("alice")).unwrap_err();
        assert!(matches!(err, Error::BackendUnavailable { .. }));
    }
}
