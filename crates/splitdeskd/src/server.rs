use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use splitdesk_core::{
    detect_host_os, CreateSessionRequest, Error as CoreError, HostOs, Resolution, SessionId,
    SessionStatus, SessionSupport, UserName,
};
#[cfg(windows)]
use splitdesk_media::{CaptureBackend, CaptureRequest};
use splitdesk_protocol::DEFAULT_MEDIA_BIND;
use splitdesk_session::{SessionBackend, SessionManager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::auth::authorize;
use crate::diagnostics::{gpu_diagnostics, media_path_for, probe_capabilities};
use crate::ipc::{
    DaemonCommand, DaemonRequest, DaemonResponse, DaemonResult, DiagnosticsScope, StageSample,
    DEFAULT_BIND,
};
use crate::media::{serve_media, FrameHub};
use crate::token::{generate_token, write_token_file, Token};
use crate::DEFAULT_IDLE_SECS;

pub struct DaemonConfig {
    pub bind: String,
    pub token_path: PathBuf,
    pub idle: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            token_path: crate::default_token_path(),
            idle: Duration::from_secs(DEFAULT_IDLE_SECS),
        }
    }
}

struct SessionWorker {
    task: JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

struct DaemonState {
    bind: String,
    token: Token,
    manager: Arc<SessionManager>,
    frames: Arc<FrameHub>,
    workers: Mutex<HashMap<SessionId, SessionWorker>>,
}

pub async fn run_daemon(config: DaemonConfig) -> anyhow::Result<()> {
    let token = generate_token();
    write_token_file(&config.token_path, &token)?;
    let control_addr: std::net::SocketAddr = config.bind.parse()?;
    anyhow::ensure!(
        control_addr.ip().is_loopback(),
        "control bind must be a loopback address"
    );
    let listener = TcpListener::bind(control_addr).await?;
    let media_listener = TcpListener::bind(DEFAULT_MEDIA_BIND).await?;
    tracing::info!(
        bind = %config.bind,
        token_path = %config.token_path.display(),
        "splitdeskd listening"
    );

    let backend = host_backend();
    let os = detect_host_os();
    let support = match os {
        HostOs::Linux => SessionSupport::LinuxMultiUser,
        HostOs::Windows => SessionSupport::WindowsSingleInteractive,
    };
    let manager = Arc::new(SessionManager::with_backend(
        os,
        support,
        Some(config.idle),
        backend,
    ));
    let frames = Arc::new(FrameHub::new());
    let media_token = Arc::new(token.clone());
    let state = Arc::new(DaemonState {
        bind: config.bind.clone(),
        token,
        manager: Arc::clone(&manager),
        frames: Arc::clone(&frames),
        workers: Mutex::new(HashMap::new()),
    });
    tracing::info!(bind = DEFAULT_MEDIA_BIND, "SplitDesk media listening");
    tokio::spawn(serve_media(media_listener, media_token, manager, frames));

    let reaper = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            match reaper.manager.reap_idle() {
                Ok(ids) => {
                    for id in &ids {
                        abort_worker(&reaper, *id);
                    }
                    if !ids.is_empty() {
                        tracing::info!(count = ids.len(), "reaped idle sessions");
                    }
                }
                Err(err) => tracing::warn!(error = %err, "idle reap failed"),
            }
        }
    });

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                tracing::info!(peer = %addr, "control connection");
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(err) = handle_connection(state, stream).await {
                        tracing::warn!(error = %err, "control connection closed");
                    }
                });
            }
            Err(err) => {
                tracing::error!(error = %err, "accept failed");
            }
        }
    }
}

fn host_backend() -> Arc<dyn SessionBackend> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(splitdesk_host_linux::LinuxSessionBackend::new())
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(splitdesk_host_windows::WindowsSessionBackend::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        compile_error!("SplitDesk daemon requires Linux or Windows");
    }
}

async fn handle_connection(state: Arc<DaemonState>, stream: TcpStream) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = dispatch_line(&state, &line).await;
        let mut payload = serde_json::to_string(&response)?;
        payload.push('\n');
        writer.write_all(payload.as_bytes()).await?;
        writer.flush().await?;
    }
    Ok(())
}

async fn dispatch_line(state: &DaemonState, line: &str) -> DaemonResponse {
    let req: DaemonRequest = match serde_json::from_str(line) {
        Ok(req) => req,
        Err(err) => return DaemonResponse::fail(0, "Protocol", err.to_string()),
    };
    if let Err(fail) = authorize(&req.token, state.token.as_str()) {
        tracing::warn!(req_id = req.req_id, "auth failed");
        return DaemonResponse::fail(req.req_id, fail.code(), fail.message());
    }
    match handle_command(state, req.cmd) {
        Ok(result) => DaemonResponse::ok(req.req_id, result),
        Err(err) => DaemonResponse::fail(req.req_id, error_code(&err), err.to_string()),
    }
}

fn handle_command(state: &DaemonState, cmd: DaemonCommand) -> Result<DaemonResult, CoreError> {
    match cmd {
        DaemonCommand::Status => {
            let sessions = state.manager.list_sessions()?;
            Ok(DaemonResult::Status {
                bind: state.bind.clone(),
                host_os: detect_host_os(),
                session_count: sessions.len(),
                capabilities: probe_capabilities(),
            })
        }
        DaemonCommand::SessionCreate {
            user,
            width,
            height,
            fps,
            memory_limit,
            cpu_affinity,
        } => {
            let mut req = CreateSessionRequest::new(UserName::new(user));
            req.resolution = Resolution::new(width.unwrap_or(1920), height.unwrap_or(1080));
            req.fps = fps.unwrap_or(60);
            req.memory_limit = memory_limit;
            req.cpu_affinity = cpu_affinity;
            let record = state.manager.create_session(req)?;
            spawn_session_worker(state, record.id);
            Ok(DaemonResult::Session {
                session: record,
                media_bind: Some(DEFAULT_MEDIA_BIND.to_string()),
            })
        }
        DaemonCommand::SessionDestroy { id } => {
            let sid = parse_session_id(&id)?;
            abort_worker(state, sid);
            state.manager.destroy_session(&sid)?;
            Ok(DaemonResult::Ok)
        }
        DaemonCommand::SessionList => {
            let sessions = state.manager.list_sessions()?;
            Ok(DaemonResult::SessionList { sessions })
        }
        DaemonCommand::SessionInfo { id } => {
            let sid = parse_session_id(&id)?;
            let session = state.manager.info(&sid)?;
            let media_bind = matches!(session.status, SessionStatus::Connected)
                .then(|| DEFAULT_MEDIA_BIND.to_string());
            Ok(DaemonResult::Session {
                session,
                media_bind,
            })
        }
        DaemonCommand::Metrics { id } => Ok(empty_metrics(id)),
        DaemonCommand::Diagnostics { scope } => Ok(diagnostics(state, scope)),
        DaemonCommand::Attach { id } => {
            let sid = parse_session_id(&id)?;
            let session = state.manager.attach(&sid)?;
            Ok(DaemonResult::Session {
                session,
                media_bind: Some(DEFAULT_MEDIA_BIND.to_string()),
            })
        }
        DaemonCommand::Detach { id } => {
            let sid = parse_session_id(&id)?;
            let session = state.manager.detach(&sid)?;
            Ok(DaemonResult::Session {
                session,
                media_bind: None,
            })
        }
        DaemonCommand::Disconnect { id } => {
            tracing::info!(session_id = %id, "client disconnect; releasing virtual keys/buttons");
            let sid = parse_session_id(&id)?;
            let session = state.manager.disconnect(&sid)?;
            Ok(DaemonResult::Session {
                session,
                media_bind: None,
            })
        }
    }
}

fn diagnostics(state: &DaemonState, scope: DiagnosticsScope) -> DaemonResult {
    let caps = probe_capabilities();
    let live_capture = state.frames.has_live_capture();
    match scope {
        DiagnosticsScope::All => DaemonResult::Diagnostics {
            capabilities: Some(caps.clone()),
            gpu: Some(gpu_diagnostics()),
            media_path: Some(media_path_for(&caps, live_capture)),
        },
        DiagnosticsScope::Gpu => DaemonResult::Diagnostics {
            capabilities: None,
            gpu: Some(gpu_diagnostics()),
            media_path: None,
        },
        DiagnosticsScope::MediaPath => DaemonResult::Diagnostics {
            capabilities: None,
            gpu: None,
            media_path: Some(media_path_for(&caps, live_capture)),
        },
    }
}

fn empty_metrics(session_id: Option<String>) -> DaemonResult {
    let empty = splitdesk_metrics::LatencyStats::empty();
    let stages = splitdesk_metrics::Stage::ALL
        .iter()
        .map(|stage| StageSample {
            stage: stage.as_str().to_string(),
            clock: format!("{:?}", splitdesk_metrics::ClockDomain::ServerLocal),
            mean: empty.mean,
            median: empty.median,
            p95: empty.p95,
            p99: empty.p99,
            max: empty.max,
            n: empty.n,
            estimated: false,
        })
        .collect();
    DaemonResult::Metrics { session_id, stages }
}

fn spawn_session_worker(state: &DaemonState, id: SessionId) {
    let stop = Arc::new(AtomicBool::new(false));
    let task_stop = Arc::clone(&stop);
    let slot = state.frames.register(id);
    let task = tokio::spawn(async move {
        tracing::info!(session_id = %id, "session worker start");
        #[cfg(windows)]
        let _capture_thread = {
            let capture_stop = Arc::clone(&task_stop);
            match std::thread::Builder::new()
                .name(format!("splitdesk-capture-{id}"))
                .spawn(move || run_capture_worker(id, capture_stop, slot))
            {
                Ok(thread) => Some(thread),
                Err(err) => {
                    tracing::error!(session_id = %id, error = %err, "capture thread spawn failed");
                    None
                }
            }
        };
        #[cfg(not(windows))]
        let _ = slot;

        while !task_stop.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    state
        .workers
        .lock()
        .insert(id, SessionWorker { task, stop });
}

#[cfg(windows)]
fn run_capture_worker(
    id: SessionId,
    stop: Arc<AtomicBool>,
    slot: Arc<crate::media::SessionFrameSlot>,
) {
    let mut capture = splitdesk_host_windows::DxgiDuplicationCapture::new();
    let request = CaptureRequest {
        resolution: Resolution::new(1920, 1080),
        fps: 60,
    };
    if let Err(err) = capture.start(&request) {
        tracing::error!(session_id = %id, error = %err, "DXGI capture start failed");
        return;
    }
    slot.set_live_capture(true);
    while !stop.load(Ordering::Acquire) {
        match capture.capture_bgra() {
            Ok(Some(frame)) => slot.push(frame),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(session_id = %id, error = %err, "DXGI capture failed");
                break;
            }
        }
    }
    slot.set_live_capture(false);
    if let Err(err) = capture.stop() {
        tracing::warn!(session_id = %id, error = %err, "DXGI capture stop failed");
    }
}

fn abort_worker(state: &DaemonState, id: SessionId) {
    state.frames.remove(&id);
    if let Some(worker) = state.workers.lock().remove(&id) {
        worker.stop.store(true, Ordering::Release);
        worker.task.abort();
    }
}

fn parse_session_id(id: &str) -> Result<SessionId, CoreError> {
    id.parse().map_err(|_| CoreError::Protocol)
}

fn error_code(err: &CoreError) -> &'static str {
    match err {
        CoreError::MultiUserNotSupportedByHostOs => "MultiUserNotSupportedByHostOs",
        CoreError::UserNotFound => "UserNotFound",
        CoreError::SessionNotFound => "SessionNotFound",
        CoreError::PermissionDenied => "PermissionDenied",
        CoreError::BackendUnavailable { .. } => "BackendUnavailable",
        CoreError::IsolationViolation => "IsolationViolation",
        CoreError::Protocol => "Protocol",
        CoreError::Io => "Io",
        CoreError::Auth => "Auth",
        CoreError::Unsupported => "Unsupported",
    }
}
