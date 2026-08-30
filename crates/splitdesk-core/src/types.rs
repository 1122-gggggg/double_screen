use crate::UserName;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub const HD1080: Self = Self {
        width: 1920,
        height: 1080,
    };

    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl Default for Resolution {
    fn default() -> Self {
        Self::HD1080
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostOs {
    Linux,
    Windows,
    MacOs,
    #[serde(other)]
    Unknown,
}

pub fn detect_host_os() -> HostOs {
    if cfg!(target_os = "windows") {
        HostOs::Windows
    } else if cfg!(target_os = "macos") {
        HostOs::MacOs
    } else if cfg!(target_os = "linux") {
        HostOs::Linux
    } else {
        HostOs::Unknown
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionStatus {
    Starting,
    Running,
    Detached,
    Connected,
    Stopping,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaptureKind {
    PipeWire,
    Dxgi,
    WindowsGraphicsCapture,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EncoderKind {
    Nvenc,
    Vaapi,
    Qsv,
    Amf,
    SoftwareFallback,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompositorKind {
    Weston,
    Wlroots,
    WindowsDwm,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionSupport {
    LinuxMultiUser,
    WindowsSingleInteractive,
    WindowsServerRds,
    #[serde(other)]
    UnsupportedHost,
}

pub const DEFAULT_FPS: u32 = 60;

fn default_fps() -> u32 {
    DEFAULT_FPS
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub user: UserName,
    #[serde(default)]
    pub resolution: Resolution,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default)]
    pub memory_limit: Option<String>,
    #[serde(default)]
    pub cpu_affinity: Option<String>,
}

impl CreateSessionRequest {
    pub fn new(user: impl Into<UserName>) -> Self {
        Self {
            user: user.into(),
            resolution: Resolution::default(),
            fps: DEFAULT_FPS,
            memory_limit: None,
            cpu_affinity: None,
        }
    }
}

impl Default for CreateSessionRequest {
    fn default() -> Self {
        Self::new(UserName::new(""))
    }
}
