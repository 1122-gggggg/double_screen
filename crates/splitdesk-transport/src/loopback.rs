use crate::backend::{TransportBackend, TransportKind};
use crate::demux::{InputDemux, ReliableQueue};
use splitdesk_core::Error;
use splitdesk_media::{BoundedSlot, Frame, InputEvent};
use std::sync::Arc;

const CONTROL_CAP: usize = 32;

enum Wiring {
    Loop {
        media: BoundedSlot<Frame>,
        input: InputDemux,
        control: ReliableQueue<Vec<u8>>,
    },
    Paired {
        out_media: Arc<BoundedSlot<Frame>>,
        in_media: Arc<BoundedSlot<Frame>>,
        out_input: Arc<InputDemux>,
        in_input: Arc<InputDemux>,
        out_ctrl: Arc<ReliableQueue<Vec<u8>>>,
        in_ctrl: Arc<ReliableQueue<Vec<u8>>>,
    },
}

/// In-process Phase 1 transport. Video is latest-wins; keys/buttons are reliable.
pub struct LoopbackTransport {
    wiring: Wiring,
}

impl LoopbackTransport {
    pub fn new() -> Self {
        Self {
            wiring: Wiring::Loop {
                media: BoundedSlot::new(),
                input: InputDemux::new(),
                control: ReliableQueue::new(CONTROL_CAP),
            },
        }
    }

    pub fn pair() -> (Self, Self) {
        let media_ab = Arc::new(BoundedSlot::new());
        let media_ba = Arc::new(BoundedSlot::new());
        let input_ab = Arc::new(InputDemux::new());
        let input_ba = Arc::new(InputDemux::new());
        let ctrl_ab = Arc::new(ReliableQueue::new(CONTROL_CAP));
        let ctrl_ba = Arc::new(ReliableQueue::new(CONTROL_CAP));
        let a = Self {
            wiring: Wiring::Paired {
                out_media: Arc::clone(&media_ab),
                in_media: Arc::clone(&media_ba),
                out_input: Arc::clone(&input_ab),
                in_input: Arc::clone(&input_ba),
                out_ctrl: Arc::clone(&ctrl_ab),
                in_ctrl: Arc::clone(&ctrl_ba),
            },
        };
        let b = Self {
            wiring: Wiring::Paired {
                out_media: media_ba,
                in_media: media_ab,
                out_input: input_ba,
                in_input: input_ab,
                out_ctrl: ctrl_ba,
                in_ctrl: ctrl_ab,
            },
        };
        (a, b)
    }
}

impl Default for LoopbackTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TransportBackend for LoopbackTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Loopback
    }

    fn send_media(&self, frame: Frame) -> Result<(), Error> {
        match &self.wiring {
            Wiring::Loop { media, .. } => {
                media.push(frame);
            }
            Wiring::Paired { out_media, .. } => {
                out_media.push(frame);
            }
        }
        Ok(())
    }

    fn recv_media(&self) -> Result<Option<Frame>, Error> {
        Ok(match &self.wiring {
            Wiring::Loop { media, .. } => media.take(),
            Wiring::Paired { in_media, .. } => in_media.take(),
        })
    }

    fn send_input(&self, event: InputEvent) -> Result<(), Error> {
        match &self.wiring {
            Wiring::Loop { input, .. } => {
                input.push(event)?;
            }
            Wiring::Paired { out_input, .. } => {
                out_input.push(event)?;
            }
        }
        Ok(())
    }

    fn recv_input(&self) -> Result<Option<InputEvent>, Error> {
        Ok(match &self.wiring {
            Wiring::Loop { input, .. } => input.pop(),
            Wiring::Paired { in_input, .. } => in_input.pop(),
        })
    }

    fn send_control(&self, bytes: Vec<u8>) -> Result<(), Error> {
        match &self.wiring {
            Wiring::Loop { control, .. } => control.push(bytes),
            Wiring::Paired { out_ctrl, .. } => out_ctrl.push(bytes),
        }
    }

    fn recv_control(&self) -> Result<Option<Vec<u8>>, Error> {
        Ok(match &self.wiring {
            Wiring::Loop { control, .. } => control.pop(),
            Wiring::Paired { in_ctrl, .. } => in_ctrl.pop(),
        })
    }

    fn close(&self) -> Result<(), Error> {
        match &self.wiring {
            Wiring::Loop {
                media,
                input,
                control,
            } => {
                media.clear();
                input.clear();
                control.clear();
            }
            Wiring::Paired {
                in_media,
                in_input,
                in_ctrl,
                ..
            } => {
                in_media.clear();
                in_input.clear();
                in_ctrl.clear();
            }
        }
        Ok(())
    }
}
