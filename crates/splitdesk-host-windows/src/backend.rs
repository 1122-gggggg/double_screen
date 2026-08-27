use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use splitdesk_core::{
    CaptureKind, CreateSessionRequest, EncoderKind, Error, HostOs, SessionId, SessionStatus,
    SessionSupport, UserName,
};
use splitdesk_session::{SessionBackend, SessionRecord};

use crate::detect::detect_windows_capabilities;
#[cfg(windows)]
use crate::WINDOWS_SESSION_0;

struct LiveSession {
    record: SessionRecord,
}

/// In-process Windows session map. Does not create extra Windows logons.
pub struct WindowsSessionBackend {
    support: SessionSupport,
    encoder: EncoderKind,
    next_id: AtomicU32,
    sessions: Mutex<HashMap<SessionId, LiveSession>>,
}

impl WindowsSessionBackend {
    pub fn new() -> Self {
        let caps = detect_windows_capabilities();
        Self {
            support: caps.session_support,
            encoder: caps.encoder,
            next_id: AtomicU32::new(1),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn allocate_id(&self) -> SessionId {
        loop {
            let n = self.next_id.fetch_add(1, Ordering::Relaxed);
            let id = SessionId::new(n);
            if !self.sessions.lock().contains_key(&id) {
                return id;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn occupied_single_interactive() -> Self {
        let backend = Self {
            support: SessionSupport::WindowsSingleInteractive,
            encoder: EncoderKind::Unavailable,
            next_id: AtomicU32::new(2),
            sessions: Mutex::new(HashMap::new()),
        };
        let id = SessionId::new(1);
        backend.sessions.lock().insert(
            id,
            LiveSession {
                record: SessionRecord {
                    id,
                    user: UserName::from("alice"),
                    os: HostOs::Windows,
                    resolution: splitdesk_core::Resolution::default(),
                    fps: 60,
                    status: SessionStatus::Running,
                    wayland_display: None,
                    windows_session_id: Some(1),
                    encoder: EncoderKind::Unavailable,
                    capture: CaptureKind::Dxgi,
                },
            },
        );
        backend
    }

    fn counts_as_interactive(status: SessionStatus) -> bool {
        matches!(
            status,
            SessionStatus::Starting
                | SessionStatus::Running
                | SessionStatus::Detached
                | SessionStatus::Connected
        )
    }

    fn refuse_second_interactive(
        &self,
        map: &HashMap<SessionId, LiveSession>,
    ) -> Result<(), Error> {
        if self.support != SessionSupport::WindowsSingleInteractive {
            return Ok(());
        }
        if map
            .values()
            .any(|s| Self::counts_as_interactive(s.record.status))
        {
            return Err(Error::MultiUserNotSupportedByHostOs);
        }
        Ok(())
    }

    fn isolation_ok(
        map: &HashMap<SessionId, LiveSession>,
        user: &UserName,
        wts_id: u32,
    ) -> Result<(), Error> {
        for row in map.values() {
            if row.record.windows_session_id == Some(wts_id) && &row.record.user != user {
                return Err(Error::IsolationViolation);
            }
        }
        Ok(())
    }
}

impl Default for WindowsSessionBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionBackend for WindowsSessionBackend {
    fn create_session(&self, request: CreateSessionRequest) -> Result<SessionRecord, Error> {
        let mut map = self.sessions.lock();
        self.refuse_second_interactive(&map)?;

        let bind = bind_interactive_user(&request.user)?;
        Self::isolation_ok(&map, &request.user, bind.windows_session_id)?;

        let id = {
            drop(map);
            let id = self.allocate_id();
            map = self.sessions.lock();
            self.refuse_second_interactive(&map)?;
            id
        };

        let fps = if request.fps == 0 { 60 } else { request.fps };
        let mut record = SessionRecord {
            id,
            user: request.user,
            os: HostOs::Windows,
            resolution: request.resolution,
            fps,
            status: SessionStatus::Starting,
            wayland_display: None,
            windows_session_id: Some(bind.windows_session_id),
            encoder: self.encoder,
            capture: CaptureKind::Dxgi,
        };
        record.status = SessionStatus::Running;
        map.insert(
            id,
            LiveSession {
                record: record.clone(),
            },
        );
        Ok(record)
    }

    fn destroy_session(&self, id: &SessionId) -> Result<(), Error> {
        let mut map = self.sessions.lock();
        match map.remove(id) {
            Some(_) => Ok(()),
            None => Err(Error::SessionNotFound),
        }
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

struct Bind {
    windows_session_id: u32,
}

fn bind_interactive_user(user: &UserName) -> Result<Bind, Error> {
    #[cfg(not(windows))]
    {
        let _ = user;
        Err(Error::BackendUnavailable {
            detail: "Windows session backend cannot spawn or bind WTS sessions on this host OS"
                .into(),
        })
    }
    #[cfg(windows)]
    {
        windows_bind(user)
    }
}

#[cfg(windows)]
fn windows_bind(user: &UserName) -> Result<Bind, Error> {
    use crate::wts::{
        active_console_session_id, current_process_session_id, enumerate_sessions_ex,
        WtsSessionState,
    };

    let wanted = user.as_str();
    if wanted.is_empty() {
        return Err(Error::UserNotFound);
    }

    let sessions =
        enumerate_sessions_ex().map_err(|detail| Error::BackendUnavailable { detail })?;
    let match_user = |name: &str| name.eq_ignore_ascii_case(wanted);

    let interactive = sessions.iter().find(|s| {
        match_user(&s.user_name)
            && s.session_id != WINDOWS_SESSION_0
            && matches!(
                s.state,
                WtsSessionState::Active | WtsSessionState::Connected
            )
    });

    let chosen = if let Some(found) = interactive {
        found.session_id
    } else {
        let console = active_console_session_id().unwrap_or(current_process_session_id());
        if console == WINDOWS_SESSION_0 {
            return Err(Error::BackendUnavailable {
                detail:
                    "no interactive WTS session: Session 0 is isolated and cannot host DXGI capture"
                        .into(),
            });
        }
        let console_row = sessions.iter().find(|s| s.session_id == console);
        match console_row {
            Some(row) if row.user_name.is_empty() || match_user(&row.user_name) => console,
            Some(_) => return Err(Error::MultiUserNotSupportedByHostOs),
            None => {
                if current_user_matches(wanted) {
                    console
                } else {
                    return Err(Error::UserNotFound);
                }
            }
        }
    };

    if chosen == WINDOWS_SESSION_0 {
        return Err(Error::BackendUnavailable {
            detail: "refusing to bind capture to Session 0 (non-interactive service session)"
                .into(),
        });
    }

    if !current_user_matches(wanted) {
        let logged_on = sessions.iter().any(|s| match_user(&s.user_name));
        if !logged_on {
            return Err(Error::UserNotFound);
        }
        if detect_windows_capabilities().session_support == SessionSupport::WindowsSingleInteractive
            && !console_is_user(wanted)
        {
            return Err(Error::MultiUserNotSupportedByHostOs);
        }
    }

    Ok(Bind {
        windows_session_id: chosen,
    })
}

#[cfg(windows)]
fn current_user_matches(wanted: &str) -> bool {
    current_user_name()
        .map(|n| n.eq_ignore_ascii_case(wanted))
        .unwrap_or(false)
}

#[cfg(windows)]
fn console_is_user(wanted: &str) -> bool {
    use crate::wts::{active_console_session_id, enumerate_sessions_ex};
    let Ok(console) = active_console_session_id() else {
        return false;
    };
    let Ok(sessions) = enumerate_sessions_ex() else {
        return false;
    };
    sessions
        .iter()
        .any(|s| s.session_id == console && s.user_name.eq_ignore_ascii_case(wanted))
}

#[cfg(windows)]
fn current_user_name() -> Option<String> {
    std::env::var("USERNAME").ok().filter(|s| !s.is_empty())
}
