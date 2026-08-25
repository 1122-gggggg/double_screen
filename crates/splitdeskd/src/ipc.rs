use serde::{Deserialize, Serialize};
use splitdesk_core::{Capabilities, MemoryPath};
use splitdesk_session::SessionRecord;

pub const DEFAULT_BIND: &str = "127.0.0.1:9823";

#[derive(Clone, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub req_id: u64,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(flatten)]
    pub cmd: DaemonCommand,
}

impl std::fmt::Debug for DaemonRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonRequest")
            .field("req_id", &self.req_id)
            .field("token", &"<redacted>")
            .field("cmd", &self.cmd)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DaemonCommand {
    Status,
    SessionCreate {
        user: String,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        fps: Option<u32>,
        #[serde(default)]
        memory_limit: Option<String>,
        #[serde(default)]
        cpu_affinity: Option<String>,
    },
    SessionDestroy {
        id: String,
    },
    SessionList,
    SessionInfo {
        id: String,
    },
    Metrics {
        #[serde(default)]
        id: Option<String>,
    },
    Diagnostics {
        #[serde(default)]
        scope: DiagnosticsScope,
    },
    Attach {
        id: String,
    },
    Detach {
        id: String,
    },
    Disconnect {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticsScope {
    #[default]
    All,
    Gpu,
    MediaPath,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub req_id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DaemonErrorBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<DaemonResult>,
}

impl DaemonResponse {
    pub fn ok(req_id: u64, result: DaemonResult) -> Self {
        Self {
            req_id,
            ok: true,
            error: None,
            result: Some(result),
        }
    }

    pub fn fail(req_id: u64, code: &str, message: impl Into<String>) -> Self {
        Self {
            req_id,
            ok: false,
            error: Some(DaemonErrorBody {
                code: code.to_string(),
                message: message.into(),
            }),
            result: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResult {
    Status {
        bind: String,
        host_os: splitdesk_core::HostOs,
        session_count: usize,
        capabilities: Capabilities,
    },
    Session {
        session: SessionRecord,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_bind: Option<String>,
    },
    SessionList {
        sessions: Vec<SessionRecord>,
    },
    Metrics {
        session_id: Option<String>,
        stages: Vec<StageSample>,
    },
    Diagnostics {
        capabilities: Option<Capabilities>,
        gpu: Option<serde_json::Value>,
        media_path: Option<MemoryPath>,
    },
    Ok,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageSample {
    pub stage: String,
    pub clock: String,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub n: u64,
    pub estimated: bool,
}
