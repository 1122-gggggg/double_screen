//! Local SplitDesk control daemon library: JSON-lines IPC, token file, session dispatch.

mod auth;
mod diagnostics;
mod ipc;
mod media;
mod rpc;
mod server;
mod token;

pub use auth::{authorize, AuthFailure};
pub use diagnostics::{gpu_diagnostics, media_path_for, probe_capabilities};
pub use ipc::{
    DaemonCommand, DaemonRequest, DaemonResponse, DaemonResult, DiagnosticsScope, DEFAULT_BIND,
};
pub use rpc::{rpc, rpc_with_token, token_path_hint, RpcError};
pub use server::{run_daemon, DaemonConfig};
pub use splitdesk_protocol::DEFAULT_MEDIA_BIND;
pub use token::{
    default_token_path, generate_token, load_token, write_token_file, Token, TOKEN_BYTE_LEN,
};

pub const DEFAULT_IDLE_SECS: u64 = 900;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_command() {
        let line = r#"{"req_id":1,"token":"abc","op":"status"}"#;
        let req: DaemonRequest = serde_json::from_str(line).unwrap();
        assert_eq!(req.req_id, 1);
        assert_eq!(req.token.as_deref(), Some("abc"));
        assert!(matches!(req.cmd, DaemonCommand::Status));
    }

    #[test]
    fn parse_session_create_command() {
        let line = r#"{"req_id":2,"token":"abc","op":"session_create","user":"alice","fps":60}"#;
        let req: DaemonRequest = serde_json::from_str(line).unwrap();
        match req.cmd {
            DaemonCommand::SessionCreate { user, fps, .. } => {
                assert_eq!(user, "alice");
                assert_eq!(fps, Some(60));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_session_info_does_not_clash_with_req_id() {
        let req = DaemonRequest {
            req_id: 7,
            token: Some("abc".into()),
            cmd: DaemonCommand::SessionInfo {
                id: "sd-001".into(),
            },
        };
        let line = serde_json::to_string(&req).unwrap();
        assert!(line.contains("\"req_id\":7"));
        assert!(line.contains("\"id\":\"sd-001\""));
        let parsed: DaemonRequest = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed.req_id, 7);
        match parsed.cmd {
            DaemonCommand::SessionInfo { id } => assert_eq!(id, "sd-001"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_diagnostics_media_path() {
        let line = r#"{"req_id":3,"token":"abc","op":"diagnostics","scope":"media-path"}"#;
        let req: DaemonRequest = serde_json::from_str(line).unwrap();
        match req.cmd {
            DaemonCommand::Diagnostics { scope } => {
                assert!(matches!(scope, DiagnosticsScope::MediaPath));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn missing_token_rejected() {
        let line = r#"{"req_id":1,"op":"status"}"#;
        let req: DaemonRequest = serde_json::from_str(line).unwrap();
        assert!(req.token.is_none());
        let err = authorize(&req.token, "deadbeef").unwrap_err();
        assert!(matches!(err, AuthFailure::Missing));
    }

    #[test]
    fn empty_token_rejected() {
        let line = r#"{"req_id":1,"token":"","op":"status"}"#;
        let req: DaemonRequest = serde_json::from_str(line).unwrap();
        let err = authorize(&req.token, "deadbeef").unwrap_err();
        assert!(matches!(err, AuthFailure::Missing));
    }

    #[test]
    fn wrong_token_rejected() {
        let err = authorize(&Some("nope".into()), "deadbeef").unwrap_err();
        assert!(matches!(err, AuthFailure::Mismatch));
    }

    #[test]
    fn matching_token_accepted() {
        authorize(&Some("deadbeef".into()), "deadbeef").unwrap();
    }

    #[test]
    fn default_bind_is_localhost() {
        assert_eq!(DEFAULT_BIND, "127.0.0.1:9823");
        assert!(!DEFAULT_BIND.starts_with("0.0.0.0"));
    }

    #[test]
    fn status_result_reports_the_media_endpoint() {
        let result = DaemonResult::Status {
            bind: "127.0.0.1:9823".into(),
            media_bind: Some("[::1]:19824".into()),
            host_os: splitdesk_core::HostOs::Linux,
            session_count: 0,
            capabilities: splitdesk_core::Capabilities::unprobed(splitdesk_core::HostOs::Linux),
        };

        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["media_bind"], "[::1]:19824");
    }

    #[test]
    fn request_debug_redacts_token() {
        let req = DaemonRequest {
            req_id: 9,
            token: Some("super-secret-token".into()),
            cmd: DaemonCommand::Status,
        };
        let rendered = format!("{req:?}");
        assert!(!rendered.contains("super-secret-token"));
        assert!(rendered.contains("<redacted>"));
    }
}
