#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;

use serde::{Deserialize, Serialize};
use splitdesk_core::{
    Capabilities, CaptureKind, Codec, CompositorKind, EncoderKind, HostOs, SessionSupport,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFeatures {
    pub nvidia: bool,
    pub nvenc: bool,
    pub pipewire: bool,
    pub wayland: bool,
    pub xwayland: bool,
    pub capture: CaptureKind,
    pub encoder: EncoderKind,
    pub compositor: CompositorKind,
    pub session_support: SessionSupport,
}

pub fn detect_host_features() -> HostFeatures {
    #[cfg(not(target_os = "linux"))]
    {
        HostFeatures {
            nvidia: false,
            nvenc: false,
            pipewire: false,
            wayland: false,
            xwayland: false,
            capture: CaptureKind::Unavailable,
            encoder: EncoderKind::Unavailable,
            compositor: CompositorKind::None,
            session_support: SessionSupport::LinuxMultiUser,
        }
    }

    #[cfg(target_os = "linux")]
    {
        let nvidia = nvidia_present();
        let nvenc = nvenc_present();
        let pipewire = pipewire_present();
        let wayland = which("weston").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
            || Path::new("/usr/share/wayland").exists();
        let xwayland = which("Xwayland").is_some();
        let compositor = if which("weston").is_some() {
            CompositorKind::Weston
        } else if which("sway").is_some() || which("labwc").is_some() {
            CompositorKind::Wlroots
        } else {
            CompositorKind::None
        };
        let capture = if pipewire {
            CaptureKind::PipeWire
        } else {
            CaptureKind::Unavailable
        };
        let encoder = if nvenc {
            EncoderKind::Nvenc
        } else if vaapi_present() {
            EncoderKind::Vaapi
        } else if which("ffmpeg").is_some() || which("gst-launch-1.0").is_some() {
            EncoderKind::SoftwareFallback
        } else {
            EncoderKind::Unavailable
        };
        if matches!(encoder, EncoderKind::SoftwareFallback) {
            tracing::warn!(
                "linux host encoder is SoftwareFallback; CPU copies will add capture/encode latency"
            );
        }
        HostFeatures {
            nvidia,
            nvenc,
            pipewire,
            wayland,
            xwayland,
            capture,
            encoder,
            compositor,
            session_support: SessionSupport::LinuxMultiUser,
        }
    }
}

pub fn detect_linux_capabilities() -> Capabilities {
    let features = detect_host_features();
    let mut caps = Capabilities::unprobed(HostOs::Linux);
    caps.multi_user = true;
    caps.capture = features.capture;
    caps.encoder = features.encoder;
    caps.compositor = features.compositor;
    caps.session_support = SessionSupport::LinuxMultiUser;
    caps.nvidia = features.nvidia;
    caps.nvenc = features.nvenc;
    caps.pipewire = features.pipewire;
    caps.wayland = features.wayland;
    caps.xwayland = features.xwayland;
    caps.dxgi = false;
    caps.rds = false;
    caps.codecs = if features.nvenc {
        vec![Codec::H264, Codec::Hevc]
    } else {
        vec![Codec::H264]
    };
    caps
}

#[cfg(target_os = "linux")]
pub(crate) fn which(cmd: &str) -> Option<PathBuf> {
    let path_os = std::env::var_os("PATH");
    if let Some(paths) = path_os {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(cmd);
            if is_executable(&candidate) {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe = dir.join(format!("{cmd}.exe"));
                if is_executable(&exe) {
                    return Some(exe);
                }
            }
        }
    }
    if !cfg!(windows) {
        for dir in ["/usr/bin", "/bin", "/usr/local/bin", "/usr/sbin"] {
            let candidate = Path::new(dir).join(cmd);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn run_capture(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;
    if output.stdout.is_empty() && output.stderr.is_empty() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.is_empty() {
        text = String::from_utf8_lossy(&output.stderr).into_owned();
    }
    if text.len() > 16 * 1024 {
        text.truncate(16 * 1024);
    }
    Some(text)
}

#[cfg(target_os = "linux")]
pub(crate) fn nvidia_present() -> bool {
    Path::new("/proc/driver/nvidia").exists()
        || Path::new("/dev/nvidia0").exists()
        || Path::new("/proc/driver/nvidia/version").exists()
        || which("nvidia-smi").is_some()
}

#[cfg(target_os = "linux")]
pub(crate) fn pipewire_present() -> bool {
    which("pipewire").is_some()
        || which("pw-cli").is_some()
        || Path::new("/usr/bin/pipewire").exists()
}

#[cfg(target_os = "linux")]
pub(crate) fn nvenc_present() -> bool {
    if gst_plugin_exists("nvcodec")
        || gst_plugin_exists("nvh264enc")
        || gst_plugin_exists("nvcudah264enc")
        || gst_plugin_exists("nvh264slenc")
    {
        return true;
    }
    if let Some(text) = run_capture("ffmpeg", &["-hide_banner", "-encoders"]) {
        if text.contains("h264_nvenc") || text.contains("hevc_nvenc") || text.contains("av1_nvenc")
        {
            return true;
        }
    }
    if let Some(text) = run_capture("nvidia-smi", &["-q"]) {
        let lower = text.to_ascii_lowercase();
        if lower.contains("nvenc") || lower.contains("video encoder") {
            return true;
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn vaapi_present() -> bool {
    Path::new("/dev/dri/renderD128").exists() && which("vainfo").is_some()
        || gst_plugin_exists("vaapih264enc")
        || gst_plugin_exists("vah264enc")
}

#[cfg(target_os = "linux")]
fn gst_plugin_exists(name: &str) -> bool {
    let output = Command::new("gst-inspect-1.0").arg(name).output();
    match output {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}
