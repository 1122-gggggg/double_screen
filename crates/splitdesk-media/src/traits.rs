use crate::frame::Frame;
use crate::input::InputEvent;
use crate::nvenc::NvencSettings;
use crate::report::MediaPathReport;
use serde::{Deserialize, Serialize};
use splitdesk_core::{CaptureKind, EncoderKind, Error, MemoryPath, MemoryType, Resolution};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub resolution: Resolution,
    pub fps: u32,
}

impl Default for CaptureRequest {
    fn default() -> Self {
        Self {
            resolution: Resolution::default(),
            fps: 60,
        }
    }
}

pub trait CaptureBackend: Send + Sync {
    fn kind(&self) -> CaptureKind;
    fn memory(&self) -> MemoryType;
    fn start(&mut self, request: &CaptureRequest) -> Result<(), Error>;
    fn stop(&mut self) -> Result<(), Error>;
    fn capture_frame(&mut self) -> Result<Option<Frame>, Error>;
    fn memory_path(&self) -> MemoryPath;
}

pub trait EncoderBackend: Send + Sync {
    fn kind(&self) -> EncoderKind;
    fn configure(&mut self, settings: &NvencSettings) -> Result<(), Error>;
    fn encode(&mut self, frame: &Frame) -> Result<Option<Frame>, Error>;
    fn force_idr(&mut self) -> Result<(), Error>;
    fn memory_path(&self) -> MemoryPath;
    fn copy_latency_ns(&self) -> Option<u64>;
    fn report(&self) -> MediaPathReport;
}

pub trait InputBackend: Send + Sync {
    fn motion(&mut self, x: f64, y: f64, ts: u64) -> Result<(), Error>;
    fn button(&mut self, button: u32, pressed: bool, ts: u64) -> Result<(), Error>;
    fn key(&mut self, keycode: u32, pressed: bool, modifiers: u32, ts: u64) -> Result<(), Error>;
    fn scroll(&mut self, dx: f64, dy: f64, ts: u64) -> Result<(), Error>;
    /// Disconnect must release every virtual key and button.
    fn release_all(&mut self) -> Result<(), Error>;

    fn inject(&mut self, event: InputEvent) -> Result<(), Error> {
        match event {
            InputEvent::PointerMotion { x, y, ts } => self.motion(x, y, ts),
            InputEvent::PointerButton {
                button,
                pressed,
                ts,
            } => self.button(button, pressed, ts),
            InputEvent::Key {
                keycode,
                pressed,
                modifiers,
                ts,
            } => self.key(keycode, pressed, modifiers, ts),
            InputEvent::Scroll { dx, dy, ts } => self.scroll(dx, dy, ts),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    pub shape_png_or_none: Option<Vec<u8>>,
    pub hotspot: (u32, u32),
    pub visible: bool,
    pub position: (i32, i32),
}

pub trait CursorBackend: Send + Sync {
    fn cursor(&self) -> Result<CursorState, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioPacket {
    pub timestamp_ns: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub pcm_s16le: Vec<u8>,
}

pub trait AudioBackend: Send + Sync {
    fn start(&mut self) -> Result<(), Error>;
    fn stop(&mut self) -> Result<(), Error>;
    fn pull(&mut self) -> Result<Option<AudioPacket>, Error>;
}

/// Clipboard UTF-8. Implementations must never log `text` contents.
pub trait ClipboardBackend: Send + Sync {
    fn read_utf8(&self) -> Result<Option<String>, Error>;
    fn write_utf8(&mut self, text: &str) -> Result<(), Error>;
}
