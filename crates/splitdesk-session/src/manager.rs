use crate::record::isolation_conflict;
use crate::{SessionBackend, SessionRecord};
use parking_lot::Mutex;
use splitdesk_core::{
    detect_host_os, CaptureKind, CreateSessionRequest, EncoderKind, Error, HostOs, SessionId,
    SessionStatus, SessionSupport,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SessionEntry {
    record: SessionRecord,
    last_activity: Instant,
}

struct Inner {
    next_id: u32,
    creating: u32,
    entries: HashMap<SessionId, SessionEntry>,
}

pub struct SessionManager {
    os: HostOs,
    support: SessionSupport,
    idle: Option<Duration>,
    backend: Option<Arc<dyn SessionBackend>>,
    inner: Mutex<Inner>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self::for_host(detect_host_os(), None)
    }

    pub fn for_host(os: HostOs, idle: Option<Duration>) -> Self {
        let support = match os {
            HostOs::Linux => SessionSupport::LinuxMultiUser,
            HostOs::Windows => SessionSupport::WindowsSingleInteractive,
            HostOs::MacOs | HostOs::Unknown => SessionSupport::UnsupportedHost,
        };
        Self::new_full(os, support, idle, None)
    }

    pub fn new_full(
        os: HostOs,
        support: SessionSupport,
        idle: Option<Duration>,
        backend: Option<Arc<dyn SessionBackend>>,
    ) -> Self {
        Self {
            os,
            support,
            idle,
            backend,
            inner: Mutex::new(Inner {
                next_id: 1,
                creating: 0,
                entries: HashMap::new(),
            }),
        }
    }

    pub fn with_backend(
        os: HostOs,
        support: SessionSupport,
        idle: Option<Duration>,
        backend: Arc<dyn SessionBackend>,
    ) -> Self {
        Self::new_full(os, support, idle, Some(backend))
    }

    pub fn os(&self) -> HostOs {
        self.os
    }

    pub fn session_support(&self) -> SessionSupport {
        self.support
    }

    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle
    }

    fn single_interactive(&self) -> bool {
        matches!(self.support, SessionSupport::WindowsSingleInteractive)
    }

    fn unsupported_host(&self) -> bool {
        matches!(self.os, HostOs::MacOs | HostOs::Unknown)
            || matches!(self.support, SessionSupport::UnsupportedHost)
    }

    fn committed_active(inner: &Inner) -> usize {
        inner
            .entries
            .values()
            .filter(|entry| {
                !matches!(
                    entry.record.status,
                    SessionStatus::Stopping | SessionStatus::Failed
                )
            })
            .count()
    }

    fn reserved_active(inner: &Inner) -> usize {
        inner.creating as usize + Self::committed_active(inner)
    }

    fn conflicts(inner: &Inner, incoming: &SessionRecord) -> bool {
        inner
            .entries
            .values()
            .any(|entry| isolation_conflict(&entry.record, incoming))
    }

    pub fn create_session(&self, req: CreateSessionRequest) -> Result<SessionRecord, Error> {
        if self.unsupported_host() {
            return Err(Error::backend("host OS is not supported"));
        }
        if let Some(backend) = &self.backend {
            {
                let mut inner = self.inner.lock();
                if self.single_interactive() && Self::reserved_active(&inner) >= 1 {
                    return Err(Error::MultiUserNotSupportedByHostOs);
                }
                inner.creating += 1;
            }
            let created = backend.create_session(req);
            let mut inner = self.inner.lock();
            inner.creating = inner.creating.saturating_sub(1);
            let rec = created?;
            if self.single_interactive() && Self::committed_active(&inner) >= 1 {
                drop(inner);
                let _ = backend.destroy_session(&rec.id);
                return Err(Error::MultiUserNotSupportedByHostOs);
            }
            if Self::conflicts(&inner, &rec) {
                drop(inner);
                let _ = backend.destroy_session(&rec.id);
                return Err(Error::IsolationViolation);
            }
            if rec.id.raw() >= inner.next_id {
                inner.next_id = rec.id.raw().saturating_add(1);
            }
            let mut rec = rec;
            if rec.status == SessionStatus::Starting {
                rec.status = SessionStatus::Running;
            }
            inner.entries.insert(
                rec.id,
                SessionEntry {
                    record: rec.clone(),
                    last_activity: Instant::now(),
                },
            );
            return Ok(rec);
        }

        let mut inner = self.inner.lock();
        if self.single_interactive() && Self::reserved_active(&inner) >= 1 {
            return Err(Error::MultiUserNotSupportedByHostOs);
        }
        let id = SessionId::new(inner.next_id);
        inner.next_id = inner.next_id.saturating_add(1);
        let rec = SessionRecord {
            id,
            user: req.user,
            os: self.os,
            resolution: req.resolution,
            fps: if req.fps == 0 { 60 } else { req.fps },
            status: SessionStatus::Running,
            wayland_display: match self.os {
                HostOs::Linux => Some(format!("splitdesk-{id}")),
                HostOs::Windows | HostOs::MacOs | HostOs::Unknown => None,
            },
            windows_session_id: None,
            encoder: EncoderKind::Unavailable,
            capture: CaptureKind::Unavailable,
        };
        if Self::conflicts(&inner, &rec) {
            return Err(Error::IsolationViolation);
        }
        inner.entries.insert(
            id,
            SessionEntry {
                record: rec.clone(),
                last_activity: Instant::now(),
            },
        );
        Ok(rec)
    }

    pub fn destroy_session(&self, id: &SessionId) -> Result<(), Error> {
        {
            let mut inner = self.inner.lock();
            let entry = inner.entries.get_mut(id).ok_or(Error::SessionNotFound)?;
            entry.record.status = SessionStatus::Stopping;
        }
        if let Some(backend) = &self.backend {
            backend.destroy_session(id)?;
        }
        self.inner.lock().entries.remove(id);
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, Error> {
        let inner = self.inner.lock();
        let mut list: Vec<SessionRecord> = inner
            .entries
            .values()
            .map(|entry| entry.record.clone())
            .collect();
        list.sort_by_key(|record| record.id.raw());
        Ok(list)
    }

    pub fn info(&self, id: &SessionId) -> Result<SessionRecord, Error> {
        self.inner
            .lock()
            .entries
            .get(id)
            .map(|entry| entry.record.clone())
            .ok_or(Error::SessionNotFound)
    }

    pub fn attach(&self, id: &SessionId) -> Result<SessionRecord, Error> {
        self.transition(id, SessionStatus::Connected)
    }

    pub fn detach(&self, id: &SessionId) -> Result<SessionRecord, Error> {
        self.disconnect(id)
    }

    pub fn disconnect(&self, id: &SessionId) -> Result<SessionRecord, Error> {
        self.transition(id, SessionStatus::Detached)
    }

    fn transition(&self, id: &SessionId, target: SessionStatus) -> Result<SessionRecord, Error> {
        let mut inner = self.inner.lock();
        let entry = inner.entries.get_mut(id).ok_or(Error::SessionNotFound)?;
        match entry.record.status {
            SessionStatus::Stopping | SessionStatus::Failed => return Err(Error::Unsupported),
            SessionStatus::Starting
            | SessionStatus::Running
            | SessionStatus::Detached
            | SessionStatus::Connected => {
                entry.record.status = target;
                entry.last_activity = Instant::now();
            }
        }
        Ok(entry.record.clone())
    }

    pub fn bind_wayland_display(
        &self,
        id: &SessionId,
        display: impl Into<String>,
    ) -> Result<SessionRecord, Error> {
        let display = display.into();
        let mut inner = self.inner.lock();
        let user = inner
            .entries
            .get(id)
            .ok_or(Error::SessionNotFound)?
            .record
            .user
            .clone();
        for entry in inner.entries.values() {
            if entry.record.id != *id
                && entry.record.user != user
                && entry.record.wayland_display.as_deref() == Some(display.as_str())
            {
                return Err(Error::IsolationViolation);
            }
        }
        let entry = inner.entries.get_mut(id).ok_or(Error::SessionNotFound)?;
        entry.record.wayland_display = Some(display);
        entry.last_activity = Instant::now();
        Ok(entry.record.clone())
    }

    pub fn bind_windows_session_id(
        &self,
        id: &SessionId,
        windows_session_id: u32,
    ) -> Result<SessionRecord, Error> {
        let mut inner = self.inner.lock();
        let user = inner
            .entries
            .get(id)
            .ok_or(Error::SessionNotFound)?
            .record
            .user
            .clone();
        for entry in inner.entries.values() {
            if entry.record.id != *id
                && entry.record.user != user
                && entry.record.windows_session_id == Some(windows_session_id)
            {
                return Err(Error::IsolationViolation);
            }
        }
        let entry = inner.entries.get_mut(id).ok_or(Error::SessionNotFound)?;
        entry.record.windows_session_id = Some(windows_session_id);
        entry.last_activity = Instant::now();
        Ok(entry.record.clone())
    }

    pub fn mark_failed(&self, id: &SessionId) -> Result<SessionRecord, Error> {
        let mut inner = self.inner.lock();
        let entry = inner.entries.get_mut(id).ok_or(Error::SessionNotFound)?;
        entry.record.status = SessionStatus::Failed;
        Ok(entry.record.clone())
    }

    pub fn reap_idle(&self) -> Result<Vec<SessionId>, Error> {
        let Some(idle) = self.idle else {
            return Ok(Vec::new());
        };
        let now = Instant::now();
        let stale: Vec<SessionId> = {
            let inner = self.inner.lock();
            inner
                .entries
                .values()
                .filter(|entry| {
                    !matches!(
                        entry.record.status,
                        SessionStatus::Connected | SessionStatus::Stopping
                    ) && now.duration_since(entry.last_activity) >= idle
                })
                .map(|entry| entry.record.id)
                .collect()
        };
        let mut reaped = Vec::new();
        for id in stale {
            if self.destroy_session(&id).is_ok() {
                reaped.push(id);
            }
        }
        Ok(reaped)
    }
}

impl SessionBackend for SessionManager {
    fn create_session(&self, req: CreateSessionRequest) -> Result<SessionRecord, Error> {
        SessionManager::create_session(self, req)
    }

    fn destroy_session(&self, id: &SessionId) -> Result<(), Error> {
        SessionManager::destroy_session(self, id)
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, Error> {
        SessionManager::list_sessions(self)
    }

    fn info(&self, id: &SessionId) -> Result<SessionRecord, Error> {
        SessionManager::info(self, id)
    }
}

#[cfg(test)]
struct TestBackend {
    next: std::sync::atomic::AtomicU32,
    force_display: Option<String>,
    force_wsid: Option<u32>,
    destroyed: Mutex<Vec<SessionId>>,
}

#[cfg(test)]
impl TestBackend {
    fn shared_display(display: &str) -> Arc<Self> {
        Arc::new(Self {
            next: std::sync::atomic::AtomicU32::new(1),
            force_display: Some(display.to_string()),
            force_wsid: None,
            destroyed: Mutex::new(Vec::new()),
        })
    }
}

#[cfg(test)]
impl SessionBackend for TestBackend {
    fn create_session(&self, req: CreateSessionRequest) -> Result<SessionRecord, Error> {
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(SessionRecord {
            id: SessionId::new(n),
            user: req.user,
            os: HostOs::Linux,
            resolution: req.resolution,
            fps: req.fps,
            status: SessionStatus::Starting,
            wayland_display: self.force_display.clone(),
            windows_session_id: self.force_wsid,
            encoder: EncoderKind::Unavailable,
            capture: CaptureKind::Unavailable,
        })
    }

    fn destroy_session(&self, id: &SessionId) -> Result<(), Error> {
        self.destroyed.lock().push(*id);
        Ok(())
    }

    fn list_sessions(&self) -> Result<Vec<SessionRecord>, Error> {
        Ok(Vec::new())
    }

    fn info(&self, _id: &SessionId) -> Result<SessionRecord, Error> {
        Err(Error::SessionNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitdesk_core::UserName;

    fn req(user: &str) -> CreateSessionRequest {
        CreateSessionRequest::new(UserName::new(user))
    }

    #[test]
    fn state_machine_create_disconnect_destroy() {
        let mgr = SessionManager::for_host(HostOs::Linux, None);
        let a = mgr.create_session(req("alice")).unwrap();
        let b = mgr.create_session(req("bob")).unwrap();
        assert_eq!(a.id.to_string(), "sd-001");
        assert_eq!(b.id.to_string(), "sd-002");
        assert_eq!(a.status, SessionStatus::Running);
        assert_eq!(b.status, SessionStatus::Running);
        assert_eq!(a.wayland_display.as_deref(), Some("splitdesk-sd-001"));
        assert_ne!(a.wayland_display, b.wayland_display);

        let a = mgr.attach(&a.id).unwrap();
        assert_eq!(a.status, SessionStatus::Connected);
        let a = mgr.disconnect(&a.id).unwrap();
        assert_eq!(a.status, SessionStatus::Detached);
        assert_eq!(mgr.info(&a.id).unwrap().status, SessionStatus::Detached);
        assert_eq!(mgr.list_sessions().unwrap().len(), 2);

        mgr.destroy_session(&a.id).unwrap();
        assert!(matches!(mgr.info(&a.id), Err(Error::SessionNotFound)));
        assert!(mgr.info(&b.id).is_ok());
        assert_eq!(mgr.list_sessions().unwrap().len(), 1);
    }

    #[test]
    fn windows_rejects_second_interactive_session() {
        let mgr = SessionManager::for_host(HostOs::Windows, None);
        mgr.create_session(req("alice")).unwrap();
        assert!(matches!(
            mgr.create_session(req("bob")),
            Err(Error::MultiUserNotSupportedByHostOs)
        ));
    }

    #[test]
    fn unsupported_host_rejects_create_with_backend_unavailable() {
        let mgr = SessionManager::for_host(HostOs::MacOs, None);
        assert_eq!(mgr.session_support(), SessionSupport::UnsupportedHost);
        assert!(matches!(
            mgr.create_session(req("alice")),
            Err(Error::BackendUnavailable { .. })
        ));
    }

    #[test]
    fn public_new_full_cannot_enable_unsupported_host() {
        let mgr =
            SessionManager::new_full(HostOs::MacOs, SessionSupport::LinuxMultiUser, None, None);
        assert!(matches!(
            mgr.create_session(req("alice")),
            Err(Error::BackendUnavailable { .. })
        ));
    }

    #[test]
    fn public_with_backend_cannot_enable_unknown_host() {
        let backend = TestBackend::shared_display("wayland-0");
        let mgr = SessionManager::with_backend(
            HostOs::Unknown,
            SessionSupport::WindowsSingleInteractive,
            None,
            backend,
        );
        assert!(matches!(
            mgr.create_session(req("alice")),
            Err(Error::BackendUnavailable { .. })
        ));
    }

    #[test]
    fn isolation_rejects_shared_wayland_display() {
        let mgr = SessionManager::for_host(HostOs::Linux, None);
        let a = mgr.create_session(req("alice")).unwrap();
        let b = mgr.create_session(req("bob")).unwrap();
        let display = a.wayland_display.clone().unwrap();
        assert!(matches!(
            mgr.bind_wayland_display(&b.id, display),
            Err(Error::IsolationViolation)
        ));
        mgr.bind_wayland_display(&a.id, "wayland-shared").unwrap();
        assert!(matches!(
            mgr.bind_wayland_display(&b.id, "wayland-shared"),
            Err(Error::IsolationViolation)
        ));
        mgr.bind_windows_session_id(&a.id, 7).unwrap();
        assert!(matches!(
            mgr.bind_windows_session_id(&b.id, 7),
            Err(Error::IsolationViolation)
        ));
    }

    #[test]
    fn test_backend_isolation_rolls_back() {
        let backend = TestBackend::shared_display("wayland-0");
        let mgr = SessionManager::with_backend(
            HostOs::Linux,
            SessionSupport::LinuxMultiUser,
            None,
            backend.clone(),
        );
        let first = mgr.create_session(req("alice")).unwrap();
        assert_eq!(first.status, SessionStatus::Running);
        assert!(matches!(
            mgr.create_session(req("bob")),
            Err(Error::IsolationViolation)
        ));
        assert_eq!(mgr.list_sessions().unwrap().len(), 1);
        assert_eq!(backend.destroyed.lock().as_slice(), [SessionId::new(2)]);
    }
}
