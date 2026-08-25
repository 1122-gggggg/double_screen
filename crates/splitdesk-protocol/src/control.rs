use serde::{Deserialize, Serialize};
use splitdesk_core::{CreateSessionRequest, SessionId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticsKind {
    Gpu,
    MediaPath,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Control {
    Status,
    SessionCreate {
        #[serde(flatten)]
        request: CreateSessionRequest,
    },
    SessionDestroy {
        session_id: SessionId,
    },
    SessionList,
    SessionInfo {
        session_id: SessionId,
    },
    Metrics {
        #[serde(default)]
        session_id: Option<SessionId>,
    },
    Diagnostics {
        #[serde(default)]
        kind: Option<DiagnosticsKind>,
    },
    Attach {
        session_id: SessionId,
    },
    Detach {
        session_id: SessionId,
    },
}
