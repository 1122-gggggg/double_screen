use crate::unavailable;
use serde::{Deserialize, Serialize};
use splitdesk_core::Error;
use splitdesk_media::{Frame, InputEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportKind {
    Loopback,
    WebRtc,
}

pub trait TransportBackend: Send + Sync {
    fn kind(&self) -> TransportKind;
    fn send_media(&self, frame: Frame) -> Result<(), Error>;
    fn recv_media(&self) -> Result<Option<Frame>, Error>;
    fn send_input(&self, event: InputEvent) -> Result<(), Error>;
    fn recv_input(&self) -> Result<Option<InputEvent>, Error>;
    fn send_control(&self, bytes: Vec<u8>) -> Result<(), Error>;
    fn recv_control(&self) -> Result<Option<Vec<u8>>, Error>;
    fn close(&self) -> Result<(), Error>;
}

/// Used by [`crate::WebRtcTransport`] until a real stack is linked.
pub(crate) fn webrtc_not_linked() -> Error {
    unavailable("WebRTC stack is not compiled; use LoopbackTransport for Phase 1 local prototype")
}
