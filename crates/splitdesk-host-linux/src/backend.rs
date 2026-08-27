use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use splitdesk_core::{CreateSessionRequest, Error, Resolution, SessionId};
#[cfg(target_os = "linux")]
use splitdesk_core::{HostOs, SessionStatus};
use splitdesk_session::{SessionBackend, SessionRecord};

#[cfg(target_os = "linux")]
use crate::detect::detect_host_features;

pub fn wayland_display_for(id: &SessionId) -> String {
    format!("splitdesk-{}", session_numeric(id))
}

pub fn session_numeric(id: &SessionId) -> u32 {
    id.raw()
}

pub struct LinuxSessionBackend {
    #[cfg(target_os = "linux")]
    next_id: AtomicU32,
    sessions: Mutex<HashMap<SessionId, LiveSession>>,
    users: Mutex<HashSet<u32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxSessionRuntime {
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub runtime_dir: PathBuf,
    pub session_dir: PathBuf,
    pub input_socket: PathBuf,
    pub pipewire_target: String,
    pub resolution: Resolution,
    pub fps: u32,
}

struct LiveSession {
    record: SessionRecord,
    #[cfg(target_os = "linux")]
    handle: Option<crate::spawn::CompositorHandle>,
    #[cfg(target_os = "linux")]
    runtime: Option<LinuxSessionRuntime>,
}

impl Default for LinuxSessionBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxSessionBackend {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            next_id: AtomicU32::new(1),
            sessions: Mutex::new(HashMap::new()),
            users: Mutex::new(HashSet::new()),
        }
    }

    #[cfg(target_os = "linux")]
    fn allocate_id(&self) -> SessionId {
        loop {
            let n = self.next_id.fetch_add(1, Ordering::Relaxed);
            let id = SessionId::new(n);
            if !self.sessions.lock().contains_key(&id) {
                return id;
            }
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn reserve_user(&self, uid: u32) -> Result<(), Error> {
        if self.users.lock().insert(uid) {
            Ok(())
        } else {
            Err(Error::IsolationViolation)
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn release_user(&self, uid: u32) {
        self.users.lock().remove(&uid);
    }

    pub fn runtime(&self, id: &SessionId) -> Result<LinuxSessionRuntime, Error> {
        #[cfg(target_os = "linux")]
        {
            self.sessions
                .lock()
                .get(id)
                .and_then(|session| session.runtime.clone())
                .ok_or(Error::SessionNotFound)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = id;
            Err(Error::backend(
                "Linux session runtime is unavailable on this OS",
            ))
        }
    }
}

impl SessionBackend for LinuxSessionBackend {
    fn create_session(&self, req: CreateSessionRequest) -> Result<SessionRecord, Error> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = req;
            Err(Error::BackendUnavailable {
                detail: "linux host backend not available on this OS".to_string(),
            })
        }

        #[cfg(target_os = "linux")]
        {
            create_session_linux(self, req)
        }
    }

    fn destroy_session(&self, id: &SessionId) -> Result<(), Error> {
        let mut sessions = self.sessions.lock();
        let live = sessions.remove(id).ok_or(Error::SessionNotFound)?;
        #[cfg(target_os = "linux")]
        if let Some(runtime) = live.runtime.as_ref() {
            self.release_user(runtime.uid);
        }
        #[cfg(target_os = "linux")]
        if let Some(handle) = live.handle {
            crate::spawn::terminate_handle(handle);
        }
        #[cfg(target_os = "linux")]
        if let Some(runtime) = live.runtime {
            crate::spawn::cleanup_runtime(&runtime);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = live;
        Ok(())
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, Error> {
        Ok(self
            .sessions
            .lock()
            .values()
            .map(|s| s.record.clone())
            .collect())
    }

    fn info(&self, id: &SessionId) -> Result<SessionRecord, Error> {
        self.sessions
            .lock()
            .get(id)
            .map(|s| s.record.clone())
            .ok_or(Error::SessionNotFound)
    }
}

#[cfg(target_os = "linux")]
fn create_session_linux(
    backend: &LinuxSessionBackend,
    req: CreateSessionRequest,
) -> Result<SessionRecord, Error> {
    use crate::spawn::{authorize_user, compositor_pid, resolve_user, spawn_compositor};

    let user_label = req.user.as_str();
    let resolved = resolve_user(user_label)?;
    authorize_user(&resolved)?;

    backend.reserve_user(resolved.uid)?;

    let features = detect_host_features();
    let id = backend.allocate_id();
    let wayland_display = wayland_display_for(&id);

    {
        let sessions = backend.sessions.lock();
        for live in sessions.values() {
            if live.record.wayland_display.as_deref() == Some(wayland_display.as_str()) {
                backend.release_user(resolved.uid);
                return Err(Error::IsolationViolation);
            }
        }
    }

    let mut record = SessionRecord {
        id,
        user: req.user.clone(),
        os: HostOs::Linux,
        resolution: req.resolution,
        fps: if req.fps == 0 { 60 } else { req.fps },
        status: SessionStatus::Starting,
        wayland_display: Some(wayland_display.clone()),
        windows_session_id: None,
        encoder: features.encoder,
        capture: features.capture,
    };

    let spawned = spawn_compositor(
        &resolved,
        &wayland_display,
        req.resolution,
        if req.fps == 0 { 60 } else { req.fps },
        req.memory_limit.as_deref(),
        req.cpu_affinity.as_deref(),
    );
    match spawned {
        Ok(spawned) => {
            let pid = compositor_pid(&spawned.handle);
            tracing::info!(
                session = %id,
                socket = %wayland_display,
                pid = ?pid,
                uid = resolved.uid,
                "spawned isolated headless Weston/PipeWire session (no DRM master)"
            );
            record.status = SessionStatus::Running;
            backend.sessions.lock().insert(
                id,
                LiveSession {
                    record: record.clone(),
                    handle: Some(spawned.handle),
                    runtime: Some(spawned.runtime),
                },
            );
            Ok(record)
        }
        Err(Error::BackendUnavailable { detail }) => {
            tracing::error!(session = %id, %detail, "weston spawn failed");
            backend.release_user(resolved.uid);
            Err(Error::BackendUnavailable { detail })
        }
        Err(err) => {
            backend.release_user(resolved.uid);
            Err(err)
        }
    }
}

impl Drop for LinuxSessionBackend {
    fn drop(&mut self) {
        let mut sessions = self.sessions.lock();
        #[cfg(target_os = "linux")]
        {
            for (_, live) in sessions.drain() {
                if let Some(handle) = live.handle {
                    crate::spawn::terminate_handle(handle);
                }
                if let Some(runtime) = live.runtime {
                    crate::spawn::cleanup_runtime(&runtime);
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        sessions.clear();
        self.users.lock().clear();
    }
}

#[cfg(test)]
pub(crate) fn isolation_holds(a: &SessionRecord, b: &SessionRecord) -> bool {
    if a.user.to_string() == b.user.to_string() {
        return a.wayland_display != b.wayland_display || a.id == b.id;
    }
    a.wayland_display.is_some()
        && b.wayland_display.is_some()
        && a.wayland_display != b.wayland_display
        && a.windows_session_id.is_none()
        && b.windows_session_id.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_active_session_is_reserved_per_user() {
        let backend = LinuxSessionBackend::new();
        backend.reserve_user(1000).unwrap();
        assert!(matches!(
            backend.reserve_user(1000),
            Err(Error::IsolationViolation)
        ));
        backend.reserve_user(1001).unwrap();
        backend.release_user(1000);
        backend.reserve_user(1000).unwrap();
    }
}
