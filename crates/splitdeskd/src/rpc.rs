use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::ipc::{DaemonCommand, DaemonRequest, DaemonResponse, DaemonResult, DEFAULT_BIND};
use crate::token::{default_token_path, load_token};

pub async fn rpc(cmd: DaemonCommand) -> Result<DaemonResult, RpcError> {
    let token = load_token(None).map_err(|_| RpcError::TokenRequired)?;
    rpc_with_token(DEFAULT_BIND, token.as_str(), cmd).await
}

pub async fn rpc_with_token(
    bind: &str,
    token: &str,
    cmd: DaemonCommand,
) -> Result<DaemonResult, RpcError> {
    if token.trim().is_empty() {
        return Err(RpcError::TokenRequired);
    }
    let mut stream = TcpStream::connect(bind)
        .await
        .map_err(|e| RpcError::Io(e.to_string()))?;
    let req = DaemonRequest {
        req_id: 1,
        token: Some(token.to_string()),
        cmd,
    };
    let mut line = serde_json::to_string(&req).map_err(|e| RpcError::Protocol(e.to_string()))?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .await
        .map_err(|e| RpcError::Io(e.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|e| RpcError::Io(e.to_string()))?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    let n = reader
        .read_line(&mut response_line)
        .await
        .map_err(|e| RpcError::Io(e.to_string()))?;
    if n == 0 {
        return Err(RpcError::Protocol("empty response".into()));
    }
    let resp: DaemonResponse =
        serde_json::from_str(&response_line).map_err(|e| RpcError::Protocol(e.to_string()))?;
    if !resp.ok {
        let body = resp.error;
        let (code, message) = match body {
            Some(b) => (b.code, b.message),
            None => ("Protocol".into(), "request failed".into()),
        };
        return Err(RpcError::Remote { code, message });
    }
    resp.result
        .ok_or_else(|| RpcError::Protocol("missing result".into()))
}

pub fn token_path_hint() -> String {
    default_token_path().display().to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("token required")]
    TokenRequired,
    #[error("{code}: {message}")]
    Remote { code: String, message: String },
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("io: {0}")]
    Io(String),
}
