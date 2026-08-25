mod capabilities;
mod error;
mod id;
mod memory;
mod types;

pub use capabilities::Capabilities;
pub use error::{Error, Result};
pub use id::{SessionId, UserName};
pub use memory::{MemoryPath, MemoryType};
pub use types::{
    detect_host_os, CaptureKind, Codec, CompositorKind, CreateSessionRequest, EncoderKind, HostOs,
    Resolution, SessionStatus, SessionSupport, DEFAULT_FPS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_sequential_display() {
        assert_eq!(SessionId::new(1).to_string(), "sd-001");
        assert_eq!(SessionId::new(2).to_string(), "sd-002");
        assert_eq!(SessionId::new(1000).to_string(), "sd-1000");
        assert_eq!("sd-001".parse::<SessionId>().unwrap().raw(), 1);
        assert!("sd-000".parse::<SessionId>().is_err());
        assert!("nope".parse::<SessionId>().is_err());
    }

    #[test]
    fn resolution_and_request_defaults() {
        let r = Resolution::default();
        assert_eq!(r.width, 1920);
        assert_eq!(r.height, 1080);
        let req = CreateSessionRequest::new("alice");
        assert_eq!(req.fps, 60);
        assert_eq!(req.resolution, Resolution::HD1080);
        let parsed: CreateSessionRequest = serde_json::from_str(r#"{"user":"bob"}"#).unwrap();
        assert_eq!(parsed.user.as_str(), "bob");
        assert_eq!(parsed.fps, 60);
        assert_eq!(parsed.resolution, Resolution::default());
    }

    #[test]
    fn detect_host_os_matches_target() {
        let os = detect_host_os();
        #[cfg(target_os = "windows")]
        assert_eq!(os, HostOs::Windows);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(os, HostOs::Linux);
    }

    #[test]
    fn ipc_types_roundtrip() {
        let id = SessionId::new(7);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"sd-007\"");
        assert_eq!(serde_json::from_str::<SessionId>(&json).unwrap(), id);

        let err = Error::BackendUnavailable {
            detail: "weston missing".into(),
        };
        let err_json = serde_json::to_string(&err).unwrap();
        assert_eq!(serde_json::from_str::<Error>(&err_json).unwrap(), err);

        let path = MemoryPath::system_copies(2);
        assert!(!path.describes_zero_copy());
        assert!(format!("{path}").contains("cpu_copies_per_frame=2"));
        let path_json = serde_json::to_string(&path).unwrap();
        assert_eq!(
            serde_json::from_str::<MemoryPath>(&path_json).unwrap(),
            path
        );

        let caps = Capabilities::unprobed(HostOs::Linux);
        assert!(caps.multi_user);
        assert_eq!(caps.encoder, EncoderKind::Unavailable);
        assert_eq!(caps.capture, CaptureKind::Unavailable);
    }
}
