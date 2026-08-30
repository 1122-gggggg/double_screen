use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicU32;
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
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::auth::authorize;
use crate::diagnostics::{gpu_diagnostics, media_path_for, probe_capabilities};
use crate::ipc::{
    DaemonCommand, DaemonRequest, DaemonResponse, DaemonResult, DiagnosticsScope, StageSample,
    DEFAULT_BIND,
};
#[cfg(target_os = "linux")]
use crate::media::SessionInputEndpoint;
use crate::media::{read_bounded_line, serve_media, FrameHub, MAX_JSON_LINE_BYTES};
use crate::token::{generate_token, write_token_file, Token};
use crate::DEFAULT_IDLE_SECS;

const MAX_CONTROL_CONNECTIONS: usize = 64;
const CONTROL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct DaemonConfig {
    pub bind: String,
    pub media_bind: String,
    pub token_path: PathBuf,
    pub idle: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            media_bind: DEFAULT_MEDIA_BIND.to_string(),
            token_path: crate::default_token_path(),
            idle: Duration::from_secs(DEFAULT_IDLE_SECS),
        }
    }
}

struct SessionWorker {
    task: JoinHandle<()>,
    stop: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    capture_pid: Arc<AtomicU32>,
}

struct DaemonState {
    bind: String,
    media_bind: String,
    token: Token,
    manager: Arc<SessionManager>,
    frames: Arc<FrameHub>,
    workers: Mutex<HashMap<SessionId, SessionWorker>>,
    #[cfg(target_os = "linux")]
    linux_backend: Arc<splitdesk_host_linux::LinuxSessionBackend>,
}

pub async fn run_daemon(config: DaemonConfig) -> anyhow::Result<()> {
    let control_addr = parse_loopback_bind(&config.bind).map_err(anyhow::Error::msg)?;
    let media_addr = parse_loopback_bind(&config.media_bind).map_err(anyhow::Error::msg)?;
    let listener = TcpListener::bind(control_addr).await?;
    let media_listener = TcpListener::bind(media_addr).await?;
    let control_bind = listener.local_addr()?.to_string();
    let media_bind = media_listener.local_addr()?.to_string();
    let token = generate_token();
    write_token_file(&config.token_path, &token)?;
    tracing::info!(
        bind = %control_bind,
        token_path = %config.token_path.display(),
        "splitdeskd listening"
    );

    let os = detect_host_os();
    let support = support_for_host(os);
    let host_backend = host_backend();
    let manager = Arc::new(SessionManager::new_full(
        os,
        support,
        Some(config.idle),
        host_backend.backend,
    ));
    let frames = Arc::new(FrameHub::new());
    let media_token = Arc::new(token.clone());
    let state = Arc::new(DaemonState {
        bind: control_bind,
        media_bind: media_bind.clone(),
        token,
        manager: Arc::clone(&manager),
        frames: Arc::clone(&frames),
        workers: Mutex::new(HashMap::new()),
        #[cfg(target_os = "linux")]
        linux_backend: host_backend.linux,
    });
    tracing::info!(bind = %media_bind, "SplitDesk media listening");
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

    let control_permits = Arc::new(Semaphore::new(MAX_CONTROL_CONNECTIONS));
    loop {
        match listener.accept().await {
            Ok((stream, addr)) if addr.ip().is_loopback() => {
                let Ok(permit) = control_permits.clone().try_acquire_owned() else {
                    tracing::warn!(peer = %addr, limit = MAX_CONTROL_CONNECTIONS, "control connection limit reached");
                    continue;
                };
                tracing::info!(peer = %addr, "control connection");
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(err) = handle_connection(state, stream).await {
                        tracing::warn!(error = %err, "control connection closed");
                    }
                });
            }
            Ok((_stream, addr)) => {
                tracing::warn!(peer = %addr, "rejected non-loopback control connection");
            }
            Err(err) => {
                tracing::error!(error = %err, "accept failed");
            }
        }
    }
}

fn parse_loopback_bind(value: &str) -> Result<std::net::SocketAddr, String> {
    let address = value
        .parse::<std::net::SocketAddr>()
        .map_err(|_| format!("invalid socket address: {value}"))?;
    if !address.ip().is_loopback() {
        return Err(format!("bind must be a loopback address: {value}"));
    }
    Ok(address)
}

struct HostBackend {
    backend: Option<Arc<dyn SessionBackend>>,
    #[cfg(target_os = "linux")]
    linux: Arc<splitdesk_host_linux::LinuxSessionBackend>,
}

fn host_backend() -> HostBackend {
    #[cfg(target_os = "linux")]
    {
        let linux = Arc::new(splitdesk_host_linux::LinuxSessionBackend::new());
        let backend: Arc<dyn SessionBackend> = linux.clone();
        HostBackend {
            backend: Some(backend),
            linux,
        }
    }
    #[cfg(target_os = "windows")]
    {
        HostBackend {
            backend: Some(Arc::new(
                splitdesk_host_windows::WindowsSessionBackend::new(),
            )),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        HostBackend { backend: None }
    }
}

fn support_for_host(os: HostOs) -> SessionSupport {
    match os {
        HostOs::Linux => SessionSupport::LinuxMultiUser,
        HostOs::Windows => SessionSupport::WindowsSingleInteractive,
        HostOs::MacOs | HostOs::Unknown => SessionSupport::UnsupportedHost,
    }
}

async fn handle_connection(state: Arc<DaemonState>, stream: TcpStream) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    while let Some(line) = tokio::time::timeout(
        CONTROL_IDLE_TIMEOUT,
        read_bounded_line(&mut reader, MAX_JSON_LINE_BYTES),
    )
    .await??
    {
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
                media_bind: Some(state.media_bind.clone()),
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
                media_bind: Some(state.media_bind.clone()),
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
                .then(|| state.media_bind.clone());
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
                media_bind: Some(state.media_bind.clone()),
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

#[cfg(any(target_os = "linux", windows))]
fn spawn_session_worker(state: &DaemonState, id: SessionId) {
    let stop = Arc::new(AtomicBool::new(false));
    let task_stop = Arc::clone(&stop);
    #[cfg(target_os = "linux")]
    let capture_pid = Arc::new(AtomicU32::new(0));
    #[cfg(target_os = "linux")]
    let runtime = match state.linux_backend.runtime(&id) {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(session_id = %id, %error, "Linux session runtime missing");
            mark_session_failed(&state.manager, id);
            return;
        }
    };
    #[cfg(target_os = "linux")]
    let input = Some(SessionInputEndpoint {
        socket: runtime.input_socket.clone(),
        uid: runtime.uid,
    });
    #[cfg(not(target_os = "linux"))]
    let input = None;
    let slot = state.frames.register(id, input);
    #[cfg(target_os = "linux")]
    let task_pid = Arc::clone(&capture_pid);
    let manager = Arc::clone(&state.manager);
    let task = tokio::spawn(async move {
        tracing::info!(session_id = %id, "session worker start");
        #[cfg(windows)]
        let _capture_thread = {
            let capture_stop = Arc::clone(&task_stop);
            let capture_manager = Arc::clone(&manager);
            let failure_manager = Arc::clone(&manager);
            match std::thread::Builder::new()
                .name(format!("splitdesk-capture-{id}"))
                .spawn(move || run_capture_worker(id, capture_stop, slot, capture_manager))
            {
                Ok(thread) => Some(thread),
                Err(err) => {
                    tracing::error!(session_id = %id, error = %err, "capture thread spawn failed");
                    mark_session_failed(&failure_manager, id);
                    None
                }
            }
        };
        #[cfg(target_os = "linux")]
        let _capture_thread = {
            let capture_stop = Arc::clone(&task_stop);
            let failure_manager = Arc::clone(&manager);
            match std::thread::Builder::new()
                .name(format!("splitdesk-pipewire-{id}"))
                .spawn(move || {
                    run_linux_capture_worker(id, capture_stop, slot, runtime, task_pid, manager)
                }) {
                Ok(thread) => Some(thread),
                Err(err) => {
                    tracing::error!(session_id = %id, error = %err, "capture thread spawn failed");
                    mark_session_failed(&failure_manager, id);
                    None
                }
            }
        };

        while !task_stop.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    state.workers.lock().insert(
        id,
        SessionWorker {
            task,
            stop,
            #[cfg(target_os = "linux")]
            capture_pid,
        },
    );
}

#[cfg(not(any(target_os = "linux", windows)))]
fn spawn_session_worker(_state: &DaemonState, _id: SessionId) {
    // SessionManager rejects UnsupportedHost before a worker can be requested.
}

#[cfg(target_os = "linux")]
fn run_linux_capture_worker(
    id: SessionId,
    stop: Arc<AtomicBool>,
    slot: Arc<crate::media::SessionFrameSlot>,
    runtime: splitdesk_host_linux::LinuxSessionRuntime,
    capture_pid: Arc<AtomicU32>,
    manager: Arc<SessionManager>,
) {
    let mut capture = match splitdesk_host_linux::PipeWireNvencCapture::start(&runtime) {
        Ok(capture) => capture,
        Err(error) => {
            tracing::error!(session_id = %id, %error, "PipeWire/NVENC capture start failed");
            mark_session_failed(&manager, id);
            return;
        }
    };
    capture_pid.store(capture.pid(), Ordering::Release);
    while !stop.load(Ordering::Acquire) {
        match capture.next_frame() {
            Ok(Some(frame)) => {
                slot.set_live_capture(true);
                slot.push(frame);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(session_id = %id, %error, "PipeWire/NVENC capture failed");
                mark_session_failed(&manager, id);
                break;
            }
        }
    }
    slot.set_live_capture(false);
    capture_pid.store(0, Ordering::Release);
}

#[cfg(windows)]
fn run_capture_worker(
    id: SessionId,
    stop: Arc<AtomicBool>,
    slot: Arc<crate::media::SessionFrameSlot>,
    manager: Arc<SessionManager>,
) {
    let mut capture = splitdesk_host_windows::DxgiDuplicationCapture::new();
    let request = CaptureRequest {
        resolution: Resolution::new(1920, 1080),
        fps: 60,
    };
    if let Err(err) = capture.start(&request) {
        tracing::error!(session_id = %id, error = %err, "DXGI capture start failed");
        mark_session_failed(&manager, id);
        return;
    }
    slot.set_live_capture(true);
    while !stop.load(Ordering::Acquire) {
        match capture.capture_bgra() {
            Ok(Some(frame)) => slot.push(frame),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(session_id = %id, error = %err, "DXGI capture failed");
                mark_session_failed(&manager, id);
                break;
            }
        }
    }
    slot.set_live_capture(false);
    if let Err(err) = capture.stop() {
        tracing::warn!(session_id = %id, error = %err, "DXGI capture stop failed");
    }
}

#[cfg(any(target_os = "linux", windows, test))]
fn mark_session_failed(manager: &SessionManager, id: SessionId) {
    if let Err(error) = manager.mark_failed(&id) {
        tracing::warn!(session_id = %id, %error, "could not mark failed session");
    }
}

fn abort_worker(state: &DaemonState, id: SessionId) {
    state.frames.remove(&id);
    if let Some(worker) = state.workers.lock().remove(&id) {
        worker.stop.store(true, Ordering::Release);
        #[cfg(target_os = "linux")]
        {
            let pid = worker.capture_pid.load(Ordering::Acquire);
            if pid != 0 {
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_platforms_keep_control_plane_available() {
        assert_eq!(
            support_for_host(HostOs::MacOs),
            SessionSupport::UnsupportedHost
        );
        assert_eq!(
            support_for_host(HostOs::Unknown),
            SessionSupport::UnsupportedHost
        );
    }

    #[test]
    fn both_planes_require_loopback_binds() {
        assert!(parse_loopback_bind("127.0.0.1:0").is_ok());
        assert!(parse_loopback_bind("[::1]:9824").is_ok());
        assert!(parse_loopback_bind("0.0.0.0:9824").is_err());
        assert!(parse_loopback_bind("192.0.2.1:9824").is_err());
    }

    #[test]
    fn daemon_config_has_an_explicit_media_bind() {
        assert_eq!(DaemonConfig::default().media_bind, DEFAULT_MEDIA_BIND);
    }

    #[tokio::test]
    async fn rejected_bind_does_not_rotate_the_token() {
        let token_path = std::env::temp_dir().join(format!(
            "splitdesk-invalid-bind-{}.token",
            uuid::Uuid::new_v4()
        ));
        let config = DaemonConfig {
            bind: "0.0.0.0:9823".to_string(),
            token_path: token_path.clone(),
            ..DaemonConfig::default()
        };

        assert!(run_daemon(config).await.is_err());
        assert!(!token_path.exists());
    }

    #[test]
    fn capture_failure_marks_the_session_failed() {
        let manager = SessionManager::for_host(HostOs::Linux, None);
        let session = manager
            .create_session(CreateSessionRequest::new("capture-test"))
            .unwrap();

        mark_session_failed(&manager, session.id);

        assert_eq!(
            manager.info(&session.id).unwrap().status,
            SessionStatus::Failed
        );
    }
}
