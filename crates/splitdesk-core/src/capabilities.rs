use crate::{
    detect_host_os, CaptureKind, Codec, CompositorKind, EncoderKind, HostOs, SessionSupport,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub host_os: HostOs,
    pub multi_user: bool,
    pub capture: CaptureKind,
    pub encoder: EncoderKind,
    pub codecs: Vec<Codec>,
    pub compositor: CompositorKind,
    pub session_support: SessionSupport,
    pub nvidia: bool,
    pub nvenc: bool,
    pub pipewire: bool,
    pub wayland: bool,
    pub xwayland: bool,
    pub dxgi: bool,
    pub rds: bool,
}

impl Capabilities {
    pub fn unprobed(host_os: HostOs) -> Self {
        match host_os {
            HostOs::Linux => Self {
                host_os,
                multi_user: true,
                capture: CaptureKind::Unavailable,
                encoder: EncoderKind::Unavailable,
                codecs: vec![Codec::H264],
                compositor: CompositorKind::None,
                session_support: SessionSupport::LinuxMultiUser,
                nvidia: false,
                nvenc: false,
                pipewire: false,
                wayland: false,
                xwayland: false,
                dxgi: false,
                rds: false,
            },
            HostOs::Windows => Self {
                host_os,
                multi_user: false,
                capture: CaptureKind::Unavailable,
                encoder: EncoderKind::Unavailable,
                codecs: vec![Codec::H264],
                compositor: CompositorKind::None,
                session_support: SessionSupport::WindowsSingleInteractive,
                nvidia: false,
                nvenc: false,
                pipewire: false,
                wayland: false,
                xwayland: false,
                dxgi: false,
                rds: false,
            },
        }
    }

    pub fn detect_unprobed() -> Self {
        Self::unprobed(detect_host_os())
    }
}
