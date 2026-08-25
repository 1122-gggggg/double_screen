use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use splitdesk_core::{CreateSessionRequest, Error, SessionId};
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
}

struct LiveSession {
    record: SessionRecord,
    #[cfg(target_os = "linux")]
    handle: Option<crate::spawn::CompositorHandle>,
    #[cfg(target_os = "linux")]
    #[allow(dead_code)]
    fail_detail: Option<String>,
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
        if let Some(handle) = live.handle {
            crate::spawn::terminate_handle(handle);
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

    let features = detect_host_features();
    let id = backend.allocate_id();
    let wayland_display = wayland_display_for(&id);

    {
        let sessions = backend.sessions.lock();
        for live in sessions.values() {
            if live.record.wayland_display.as_deref() == Some(wayland_display.as_str()) {
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

    match spawn_compositor(
        &resolved,
        &wayland_display,
        req.resolution,
        req.memory_limit.as_deref(),
        req.cpu_affinity.as_deref(),
    ) {
        Ok(handle) => {
            let pid = compositor_pid(&handle);
            tracing::info!(
                session = %id,
                socket = %wayland_display,
                pid = ?pid,
                uid = resolved.uid,
                "spawned weston as session user (pipewire/headless, no DRM master)"
            );
            record.status = SessionStatus::Running;
            backend.sessions.lock().insert(
                id,
                LiveSession {
                    record: record.clone(),
                    handle: Some(handle),
                    fail_detail: None,
                },
            );
            Ok(record)
        }
        Err(Error::BackendUnavailable { detail }) => {
            tracing::error!(session = %id, %detail, "weston spawn failed");
            record.status = SessionStatus::Failed;
            backend.sessions.lock().insert(
                id,
                LiveSession {
                    record: record.clone(),
                    handle: None,
                    fail_detail: Some(detail),
                },
            );
            Ok(record)
        }
        Err(err) => Err(err),
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
            }
        }
        #[cfg(not(target_os = "linux"))]
        sessions.clear();
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
