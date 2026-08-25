use splitdesk_core::{
    Capabilities, CaptureKind, Codec, CompositorKind, EncoderKind, HostOs, SessionSupport,
};

/// Capabilities for a Windows 10/11 client SKU before a live GPU/RDS probe.
///
/// `multi_user` is always false: the OS allows one interactive session.
/// Inbox `mfh264enc.dll` is CPU YUV and is never advertised as `encoder`.
pub fn default_windows_client_capabilities() -> Capabilities {
    Capabilities {
        host_os: HostOs::Windows,
        multi_user: false,
        capture: CaptureKind::Dxgi,
        encoder: EncoderKind::Unavailable,
        codecs: vec![Codec::H264],
        compositor: CompositorKind::WindowsDwm,
        session_support: SessionSupport::WindowsSingleInteractive,
        nvidia: false,
        nvenc: false,
        pipewire: false,
        wayland: false,
        xwayland: false,
        dxgi: true,
        rds: false,
    }
}

/// Live probe on Windows; the Win10/11 client constructor elsewhere.
pub fn detect_windows_capabilities() -> Capabilities {
    #[cfg(windows)]
    {
        live_detect()
    }
    #[cfg(not(windows))]
    {
        default_windows_client_capabilities()
    }
}

#[cfg(windows)]
fn live_detect() -> Capabilities {
    let mut caps = default_windows_client_capabilities();
    let sku = detect_sku();
    caps.session_support = sku.session_support;
    caps.multi_user = sku.multi_user;
    caps.rds = sku.rds;

    if let Some(gpu) = probe_dxgi_adapter() {
        caps.nvidia = gpu.nvidia;
        caps.nvenc = gpu.nvenc;
        caps.encoder = gpu.encoder;
    }
    caps
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct Sku {
    session_support: SessionSupport,
    multi_user: bool,
    rds: bool,
}

/// Workstation + `VER_SUITE_SINGLEUSERTS` → one interactive session.
/// `VER_SUITE_TERMINAL` without `SINGLEUSERTS`, or `PRODUCT_SERVERRDSH`, → RDSH.
#[cfg(windows)]
fn detect_sku() -> Sku {
    let workstation = is_workstation();
    let (suite_terminal, suite_single_user_ts) = suite_mask();
    let product = product_info();

    const PRODUCT_SERVERRDSH: u32 = 0x0000_00AF;

    let rdsh = (!suite_single_user_ts && suite_terminal) || product == Some(PRODUCT_SERVERRDSH);

    if workstation && !rdsh {
        Sku {
            session_support: SessionSupport::WindowsSingleInteractive,
            multi_user: false,
            rds: false,
        }
    } else if rdsh {
        Sku {
            session_support: SessionSupport::WindowsServerRds,
            multi_user: true,
            rds: true,
        }
    } else {
        Sku {
            session_support: SessionSupport::WindowsSingleInteractive,
            multi_user: false,
            rds: false,
        }
    }
}

#[cfg(windows)]
fn is_workstation() -> bool {
    use windows::Win32::System::SystemInformation::{
        VerSetConditionMask, VerifyVersionInfoW, OSVERSIONINFOEXW, VER_PRODUCT_TYPE,
    };

    const VER_EQUAL: u8 = 1;
    const VER_NT_WORKSTATION: u8 = 1;

    let mut osvi = OSVERSIONINFOEXW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOEXW>() as u32,
        wProductType: VER_NT_WORKSTATION,
        ..Default::default()
    };
    unsafe {
        let mask = VerSetConditionMask(0, VER_PRODUCT_TYPE, VER_EQUAL);
        VerifyVersionInfoW(&mut osvi, VER_PRODUCT_TYPE, mask).is_ok()
    }
}

#[cfg(windows)]
fn suite_mask() -> (bool, bool) {
    use windows::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOEXW};

    const VER_SUITE_TERMINAL: u16 = 16;
    const VER_SUITE_SINGLEUSERTS: u16 = 256;

    let mut osvi = OSVERSIONINFOEXW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOEXW>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetVersionExW(&mut osvi as *mut OSVERSIONINFOEXW as *mut _).is_ok() };
    if !ok {
        return (false, true);
    }
    let terminal = osvi.wSuiteMask & VER_SUITE_TERMINAL != 0;
    let single = osvi.wSuiteMask & VER_SUITE_SINGLEUSERTS != 0;
    (terminal, single)
}

#[cfg(windows)]
fn product_info() -> Option<u32> {
    use windows::Win32::System::SystemInformation::{GetProductInfo, OS_PRODUCT_TYPE};

    let mut product = OS_PRODUCT_TYPE(0);
    let ok = unsafe { GetProductInfo(10, 0, 0, 0, &mut product) }.as_bool();
    if ok {
        Some(product.0)
    } else {
        None
    }
}

#[cfg(windows)]
struct GpuProbe {
    nvidia: bool,
    nvenc: bool,
    encoder: EncoderKind,
}

#[cfg(windows)]
fn probe_dxgi_adapter() -> Option<GpuProbe> {
    use crate::dxgi::{DXGI_VENDOR_AMD, DXGI_VENDOR_INTEL, DXGI_VENDOR_NVIDIA};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1};

    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0).ok()?;
        let desc = adapter.GetDesc1().ok()?;
        let vendor = desc.VendorId;
        let nvidia = vendor == DXGI_VENDOR_NVIDIA;
        let intel = vendor == DXGI_VENDOR_INTEL;
        let amd = vendor == DXGI_VENDOR_AMD;
        let nvenc = nvidia && nvenc_library_present();
        let encoder = if nvenc {
            EncoderKind::Nvenc
        } else if intel {
            EncoderKind::Qsv
        } else if amd {
            EncoderKind::Amf
        } else {
            EncoderKind::Unavailable
        };
        Some(GpuProbe {
            nvidia,
            nvenc,
            encoder,
        })
    }
}

#[cfg(windows)]
fn nvenc_library_present() -> bool {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    std::path::Path::new(&windir)
        .join("System32")
        .join("nvEncodeAPI64.dll")
        .is_file()
}
