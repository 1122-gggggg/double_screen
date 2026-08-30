#[cfg(target_os = "linux")]
use splitdesk_core::EncoderKind;
#[cfg(any(target_os = "linux", windows))]
use splitdesk_core::MemoryType;
use splitdesk_core::{Capabilities, MemoryPath};

#[cfg(not(any(target_os = "linux", windows)))]
use splitdesk_core::detect_host_os;

pub fn probe_capabilities() -> Capabilities {
    #[cfg(target_os = "linux")]
    {
        splitdesk_host_linux::detect_linux_capabilities()
    }
    #[cfg(windows)]
    {
        splitdesk_host_windows::detect_windows_capabilities()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Capabilities::unprobed(detect_host_os())
    }
}

pub fn gpu_diagnostics() -> serde_json::Value {
    #[cfg(target_os = "linux")]
    {
        serde_json::to_value(splitdesk_host_linux::diagnostics_gpu())
            .unwrap_or_else(|_| serde_json::json!({}))
    }
    #[cfg(windows)]
    {
        serde_json::json!({ "text": splitdesk_host_windows::diagnostics_gpu() })
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        serde_json::json!({ "error": "unsupported host" })
    }
}

pub fn media_path_for(caps: &Capabilities, live_capture: bool) -> MemoryPath {
    #[cfg(windows)]
    {
        let _ = caps;
        if live_capture {
            return MemoryPath {
                render_output: MemoryType::D3D11,
                capture: MemoryType::D3D11,
                conversion: MemoryType::SystemMemory,
                encoder_input: MemoryType::SystemMemory,
                cpu_copies_per_frame: 1,
            };
        }
        MemoryPath::system_copies(1)
    }
    #[cfg(target_os = "linux")]
    {
        if caps.pipewire && matches!(caps.encoder, EncoderKind::Nvenc) {
            return MemoryPath {
                render_output: MemoryType::DmaBuf,
                capture: MemoryType::DmaBuf,
                conversion: MemoryType::GlMemory,
                encoder_input: MemoryType::GlMemory,
                cpu_copies_per_frame: 0,
            };
        }
        let _ = live_capture;
        if matches!(caps.encoder, EncoderKind::SoftwareFallback) {
            tracing::warn!(
                cpu_copies_per_frame = 1u32,
                "software encoder path copies frames through system memory"
            );
        }
        MemoryPath::system_copies(1)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (caps, live_capture);
        MemoryPath::system_copies(1)
    }
}
