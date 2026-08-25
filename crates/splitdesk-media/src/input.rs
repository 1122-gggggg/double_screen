use serde::{Deserialize, Serialize};

/// Motion and scroll are latest-wins. Buttons and keys are reliable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    PointerMotion {
        x: f64,
        y: f64,
        ts: u64,
    },
    PointerButton {
        button: u32,
        pressed: bool,
        ts: u64,
    },
    Key {
        keycode: u32,
        pressed: bool,
        modifiers: u32,
        ts: u64,
    },
    Scroll {
        dx: f64,
        dy: f64,
        ts: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    LatestWins,
    Reliable,
}

impl InputEvent {
    pub fn timestamp(&self) -> u64 {
        match *self {
            Self::PointerMotion { ts, .. }
            | Self::PointerButton { ts, .. }
            | Self::Key { ts, .. }
            | Self::Scroll { ts, .. } => ts,
        }
    }

    pub fn delivery(&self) -> Delivery {
        match self {
            Self::PointerMotion { .. } | Self::Scroll { .. } => Delivery::LatestWins,
            Self::PointerButton { .. } | Self::Key { .. } => Delivery::Reliable,
        }
    }

    pub fn is_latest_wins(&self) -> bool {
        matches!(self.delivery(), Delivery::LatestWins)
    }
}

impl From<splitdesk_protocol::Input> for InputEvent {
    fn from(value: splitdesk_protocol::Input) -> Self {
        match value {
            splitdesk_protocol::Input::PointerMotion { x, y, ts } => {
                Self::PointerMotion { x, y, ts }
            }
            splitdesk_protocol::Input::PointerButton {
                button,
                pressed,
                ts,
            } => Self::PointerButton {
                button,
                pressed,
                ts,
            },
            splitdesk_protocol::Input::Key {
                keycode,
                pressed,
                modifiers,
                ts,
            } => Self::Key {
                keycode,
                pressed,
                modifiers,
                ts,
            },
            splitdesk_protocol::Input::Scroll { dx, dy, ts } => Self::Scroll { dx, dy, ts },
        }
    }
}

impl From<InputEvent> for splitdesk_protocol::Input {
    fn from(value: InputEvent) -> Self {
        match value {
            InputEvent::PointerMotion { x, y, ts } => Self::PointerMotion { x, y, ts },
            InputEvent::PointerButton {
                button,
                pressed,
                ts,
            } => Self::PointerButton {
                button,
                pressed,
                ts,
            },
            InputEvent::Key {
                keycode,
                pressed,
                modifiers,
                ts,
            } => Self::Key {
                keycode,
                pressed,
                modifiers,
                ts,
            },
            InputEvent::Scroll { dx, dy, ts } => Self::Scroll { dx, dy, ts },
        }
    }
}
