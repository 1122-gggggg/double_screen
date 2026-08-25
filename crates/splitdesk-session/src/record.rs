use serde::{Deserialize, Serialize};
use splitdesk_core::{
    CaptureKind, EncoderKind, HostOs, Resolution, SessionId, SessionStatus, UserName,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub user: UserName,
    pub os: HostOs,
    pub resolution: Resolution,
    pub fps: u32,
    pub status: SessionStatus,
    pub wayland_display: Option<String>,
    pub windows_session_id: Option<u32>,
    pub encoder: EncoderKind,
    pub capture: CaptureKind,
}

pub(crate) fn isolation_conflict(existing: &SessionRecord, incoming: &SessionRecord) -> bool {
    if existing.user == incoming.user || existing.id == incoming.id {
        return false;
    }
    if let (Some(a), Some(b)) = (&existing.wayland_display, &incoming.wayland_display) {
        if a == b {
            return true;
        }
    }
    if let (Some(a), Some(b)) = (existing.windows_session_id, incoming.windows_session_id) {
        if a == b {
            return true;
        }
    }
    false
}
