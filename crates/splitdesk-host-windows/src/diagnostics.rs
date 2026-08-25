/// Human-readable GPU / session diagnostics for `splitdesk diagnostics gpu`.
pub fn diagnostics_gpu() -> String {
    #[cfg(windows)]
    {
        windows_diagnostics()
    }
    #[cfg(not(windows))]
    {
        "Windows GPU diagnostics unavailable: this binary is not running on Windows.\n\
         DXGI Desktop Duplication (ID3D11Texture2D) is the capture path; CPU BitBlt is not used.\n\
         Inbox mfh264enc.dll is CPU YUV and is not advertised as a GPU encoder."
            .to_string()
    }
}

#[cfg(windows)]
fn windows_diagnostics() -> String {
    use crate::dxgi::{DXGI_VENDOR_AMD, DXGI_VENDOR_INTEL, DXGI_VENDOR_NVIDIA};
    use crate::wts::{active_console_session_id, current_process_session_id};
    use crate::WINDOWS_SESSION_0;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1};

    let mut lines = Vec::new();
    let proc_session = current_process_session_id();
    let console = active_console_session_id();
    lines.push(format!("process_session_id={proc_session}"));
    lines.push(format!("console_session_id={console:?}"));
    if proc_session == WINDOWS_SESSION_0 {
        lines.push(
            "capture=unavailable: process is in Session 0; DXGI DuplicateOutput must run in the interactive WTS session"
                .into(),
        );
    }

    unsafe {
        match CreateDXGIFactory1() {
            Ok(factory) => {
                let factory: IDXGIFactory1 = factory;
                let mut i = 0u32;
                while let Ok(adapter) = factory.EnumAdapters1(i) {
                    let adapter: IDXGIAdapter1 = adapter;
                    if let Ok(desc) = adapter.GetDesc1() {
                        let name = String::from_utf16_lossy(
                            &desc.Description[..desc
                                .Description
                                .iter()
                                .position(|&c| c == 0)
                                .unwrap_or(desc.Description.len())],
                        );
                        let vendor = match desc.VendorId {
                            DXGI_VENDOR_NVIDIA => "NVIDIA",
                            DXGI_VENDOR_AMD => "AMD",
                            DXGI_VENDOR_INTEL => "Intel",
                            _ => "other",
                        };
                        lines.push(format!(
                            "adapter{i} vendor={vendor} (0x{:04x}) name={name} dedicated_mib={}",
                            desc.VendorId,
                            desc.DedicatedVideoMemory / (1024 * 1024)
                        ));
                    }
                    i += 1;
                }
            }
            Err(err) => lines.push(format!("dxgi_factory=unavailable ({err})")),
        }
    }
    lines.push(
        "capture_path=DXGI DuplicateOutput -> ID3D11Texture2D (MemoryType::D3D11); CPU BitBlt is not used"
            .into(),
    );
    lines.push(
        "encoder_note=inbox mfh264enc.dll is CPU YUV; hardware MFT / NVENC is the GPU path".into(),
    );
    lines.join("\n")
}
