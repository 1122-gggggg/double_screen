use splitdesk_core::{CaptureKind, Error, MemoryPath, MemoryType, Resolution};
use splitdesk_media::{CaptureBackend, CaptureRequest, Frame};

pub const DXGI_VENDOR_NVIDIA: u32 = 0x10DE;
pub const DXGI_VENDOR_AMD: u32 = 0x1002;
pub const DXGI_VENDOR_INTEL: u32 = 0x8086;

/// DXGI_FORMAT values we accept as desktop duplication output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DxgiFormat {
    B8G8R8A8Unorm,
    R8G8B8A8Unorm,
    Unknown(u32),
}

/// NT handle / same-device D3D11 texture identity for the duplication path.
///
/// Desktop Duplication returns an `ID3D11Texture2D`. That resource is typically
/// **not** created with `D3D11_RESOURCE_MISC_SHARED_NTHANDLE`, so
/// `IDXGIResource1::CreateSharedHandle` is not assumed. `capture_frame` keeps
/// the texture on the same D3D11 device and does not CPU-map it. The explicit
/// `capture_bgra` path instead performs one staging-texture CPU readback.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct D3D11TextureHandle {
    pub width: u32,
    pub height: u32,
    pub format: DxgiFormat,
    /// NT shared handle if one was obtained; 0 means same-device texture only.
    pub nt_handle: u64,
}

/// DXGI Desktop Duplication capture. Reports [`MemoryType::D3D11`].
///
/// Until `AcquireNextFrame` has produced an `ID3D11Texture2D`, `capture_frame`
/// returns `Ok(None)` after a successful `start`, or [`Error::BackendUnavailable`]
/// if duplication cannot run (wrong session, missing DXGI, non-Windows).
pub struct DxgiDuplicationCapture {
    started: bool,
    last: Option<D3D11TextureHandle>,
    resolution: Resolution,
    frame_seq: u64,
    #[cfg(windows)]
    live: Option<LiveDuplication>,
}

#[cfg(windows)]
struct LiveDuplication {
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    duplication: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    staging: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>,
    staging_desc: Option<windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC>,
}

impl Default for DxgiDuplicationCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl DxgiDuplicationCapture {
    pub fn new() -> Self {
        Self {
            started: false,
            last: None,
            resolution: Resolution::default(),
            frame_seq: 0,
            #[cfg(windows)]
            live: None,
        }
    }

    pub fn last_handle(&self) -> Option<D3D11TextureHandle> {
        self.last
    }

    /// Acquires the next desktop frame and reads tightly packed BGRA pixels
    /// through a reusable D3D11 staging texture.
    pub fn capture_bgra(&mut self) -> Result<Option<splitdesk_protocol::MediaFrame>, Error> {
        if !self.started {
            return Err(Error::BackendUnavailable {
                detail: "DXGI Desktop Duplication has not been started".into(),
            });
        }
        #[cfg(not(windows))]
        {
            Err(Error::BackendUnavailable {
                detail: "DXGI Desktop Duplication is not compiled on this target".into(),
            })
        }
        #[cfg(windows)]
        {
            warn_cpu_readback_once();
            let live = self
                .live
                .as_mut()
                .ok_or_else(|| Error::BackendUnavailable {
                    detail: "DXGI duplication interface is not held".into(),
                })?;
            acquire_bgra(live)
        }
    }

    /// Reports the explicit GPU-to-system-memory readback used by `capture_bgra`.
    pub fn memory_path_bgra(&self) -> MemoryPath {
        MemoryPath {
            render_output: MemoryType::D3D11,
            capture: MemoryType::D3D11,
            conversion: MemoryType::SystemMemory,
            encoder_input: MemoryType::SystemMemory,
            cpu_copies_per_frame: 1,
        }
    }
}

impl CaptureBackend for DxgiDuplicationCapture {
    fn kind(&self) -> CaptureKind {
        CaptureKind::Dxgi
    }

    fn memory(&self) -> MemoryType {
        MemoryType::D3D11
    }

    fn start(&mut self, request: &CaptureRequest) -> Result<(), Error> {
        self.resolution = request.resolution;
        self.last = None;
        #[cfg(not(windows))]
        {
            self.started = false;
            return Err(Error::BackendUnavailable {
                detail: "DXGI Desktop Duplication is not compiled on this target; \
                         MemoryType::D3D11 is the intended path, CPU BitBlt is not used"
                    .into(),
            });
        }
        #[cfg(windows)]
        {
            self.live = None;
            self.started = false;
            let live = start_duplication(request.resolution)?;
            self.live = Some(live);
            self.started = true;
            Ok(())
        }
    }

    fn stop(&mut self) -> Result<(), Error> {
        self.started = false;
        self.last = None;
        #[cfg(windows)]
        {
            self.live = None;
        }
        Ok(())
    }

    fn capture_frame(&mut self) -> Result<Option<Frame>, Error> {
        if !self.started {
            return Err(Error::BackendUnavailable {
                detail: "DXGI Desktop Duplication has not been started".into(),
            });
        }
        #[cfg(not(windows))]
        {
            Err(Error::BackendUnavailable {
                detail: "DXGI Desktop Duplication is not compiled on this target".into(),
            })
        }
        #[cfg(windows)]
        {
            let live = self
                .live
                .as_ref()
                .ok_or_else(|| Error::BackendUnavailable {
                    detail: "DXGI duplication interface is not held".into(),
                })?;
            match acquire_d3d11(live)? {
                None => Ok(None),
                Some(handle) => {
                    self.last = Some(handle);
                    self.frame_seq = self.frame_seq.wrapping_add(1);
                    let id = if handle.nt_handle != 0 {
                        handle.nt_handle
                    } else {
                        self.frame_seq
                    };
                    Ok(Some(Frame::gpu(
                        server_timestamp_ns(),
                        MemoryType::D3D11,
                        handle.width,
                        handle.height,
                        id,
                    )))
                }
            }
        }
    }

    fn memory_path(&self) -> MemoryPath {
        MemoryPath {
            render_output: MemoryType::D3D11,
            capture: MemoryType::D3D11,
            conversion: MemoryType::D3D11,
            encoder_input: MemoryType::D3D11,
            cpu_copies_per_frame: 0,
        }
    }
}

#[cfg(windows)]
fn server_timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn start_duplication(resolution: Resolution) -> Result<LiveDuplication, Error> {
    use crate::wts::current_process_session_id;
    use crate::WINDOWS_SESSION_0;
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIOutput1};

    let session = current_process_session_id();
    if session == WINDOWS_SESSION_0 {
        return Err(Error::BackendUnavailable {
            detail: "DXGI Desktop Duplication cannot run in Session 0; capture must run in the interactive WTS session"
                .into(),
        });
    }

    let _ = resolution;
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| Error::BackendUnavailable {
            detail: format!("D3D11CreateDevice failed: {e}"),
        })?;
        let device = device.ok_or_else(|| Error::BackendUnavailable {
            detail: "D3D11CreateDevice returned no device".into(),
        })?;
        let context = context.ok_or_else(|| Error::BackendUnavailable {
            detail: "D3D11CreateDevice returned no immediate context".into(),
        })?;

        let dxgi_device: IDXGIDevice = device.cast().map_err(|e| Error::BackendUnavailable {
            detail: format!("ID3D11Device as IDXGIDevice failed: {e}"),
        })?;
        let adapter = dxgi_device
            .GetAdapter()
            .map_err(|e| Error::BackendUnavailable {
                detail: format!("IDXGIDevice::GetAdapter failed: {e}"),
            })?;
        let output = adapter
            .EnumOutputs(0)
            .map_err(|e| Error::BackendUnavailable {
                detail: format!("IDXGIAdapter::EnumOutputs(0) failed: {e}"),
            })?;
        let output1: IDXGIOutput1 = output.cast().map_err(|e| Error::BackendUnavailable {
            detail: format!("IDXGIOutput as IDXGIOutput1 failed: {e}"),
        })?;
        let duplication =
            output1
                .DuplicateOutput(&device)
                .map_err(|e| Error::BackendUnavailable {
                    detail: format!(
                        "IDXGIOutput1::DuplicateOutput failed ({e}); \
                     no CPU BitBlt fallback is used"
                    ),
                })?;

        Ok(LiveDuplication {
            device,
            context,
            duplication,
            staging: None,
            staging_desc: None,
        })
    }
}

#[cfg(windows)]
fn acquire_d3d11(live: &LiveDuplication) -> Result<Option<D3D11TextureHandle>, Error> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Dxgi::{
        IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    };

    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource: Option<IDXGIResource> = None;
    let acquired = unsafe {
        live.duplication
            .AcquireNextFrame(0, &mut info, &mut resource)
    };
    match acquired {
        Ok(()) => {}
        Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
        Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
            return Err(Error::BackendUnavailable {
                detail: format!("DXGI duplication access lost ({err}); recreate DuplicateOutput"),
            });
        }
        Err(err) => {
            return Err(Error::BackendUnavailable {
                detail: format!("AcquireNextFrame failed: {err}"),
            });
        }
    }

    let resource = match resource {
        Some(r) => r,
        None => return Ok(None),
    };
    let texture: ID3D11Texture2D = resource.cast().map_err(|e| Error::BackendUnavailable {
        detail: format!("duplicated IDXGIResource is not ID3D11Texture2D: {e}"),
    })?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    if desc.Width == 0 || desc.Height == 0 {
        let _ = unsafe { live.duplication.ReleaseFrame() };
        return Ok(None);
    }

    let mut owned = desc;
    owned.Usage = D3D11_USAGE_DEFAULT;
    owned.CPUAccessFlags = 0;
    owned.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    owned.MiscFlags = 0;
    owned.MipLevels = 1;
    owned.ArraySize = 1;
    owned.SampleDesc.Count = 1;
    owned.SampleDesc.Quality = 0;

    let mut dest: Option<ID3D11Texture2D> = None;
    unsafe { live.device.CreateTexture2D(&owned, None, Some(&mut dest)) }.map_err(|e| {
        Error::BackendUnavailable {
            detail: format!("CreateTexture2D GPU retain failed ({e}); CPU Map/BitBlt is not used"),
        }
    })?;
    let dest = dest.ok_or_else(|| Error::BackendUnavailable {
        detail: "CreateTexture2D returned no texture".into(),
    })?;
    unsafe {
        live.context.CopyResource(&dest, &texture);
        let _ = live.duplication.ReleaseFrame();
    }
    let _ = dest;

    let format = if desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM {
        DxgiFormat::B8G8R8A8Unorm
    } else {
        DxgiFormat::Unknown(desc.Format.0 as u32)
    };
    Ok(Some(D3D11TextureHandle {
        width: desc.Width,
        height: desc.Height,
        format,
        nt_handle: 0,
    }))
}

#[cfg(windows)]
fn warn_cpu_readback_once() {
    static WARN: std::sync::Once = std::sync::Once::new();
    WARN.call_once(|| {
        tracing::warn!(
            cpu_copies_per_frame = 1,
            "DXGI BGRA capture uses a D3D11 staging-texture CPU readback; this is not NVENC"
        );
    });
}

#[cfg(windows)]
struct DuplicationFrameGuard(windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication);

#[cfg(windows)]
impl Drop for DuplicationFrameGuard {
    fn drop(&mut self) {
        let _ = unsafe { self.0.ReleaseFrame() };
    }
}

#[cfg(windows)]
struct MappedTextureGuard<'a> {
    context: &'a windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    texture: &'a windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
}

#[cfg(windows)]
impl Drop for MappedTextureGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            self.context.Unmap(self.texture, 0);
        }
    }
}

#[cfg(windows)]
fn acquire_bgra(
    live: &mut LiveDuplication,
) -> Result<Option<splitdesk_protocol::MediaFrame>, Error> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Dxgi::{
        IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    };

    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource: Option<IDXGIResource> = None;
    let acquired = unsafe {
        live.duplication
            .AcquireNextFrame(0, &mut info, &mut resource)
    };
    match acquired {
        Ok(()) => {}
        Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
        Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
            return Err(Error::BackendUnavailable {
                detail: format!("DXGI duplication access lost ({err}); recreate DuplicateOutput"),
            });
        }
        Err(err) => {
            return Err(Error::BackendUnavailable {
                detail: format!("AcquireNextFrame failed: {err}"),
            });
        }
    }
    let _frame_guard = DuplicationFrameGuard(live.duplication.clone());

    let resource = resource.ok_or_else(|| Error::BackendUnavailable {
        detail: "AcquireNextFrame returned no desktop resource".into(),
    })?;
    let texture: ID3D11Texture2D = resource.cast().map_err(|e| Error::BackendUnavailable {
        detail: format!("duplicated IDXGIResource is not ID3D11Texture2D: {e}"),
    })?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    if desc.Width == 0 || desc.Height == 0 {
        return Ok(None);
    }
    if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
        return Err(Error::BackendUnavailable {
            detail: format!(
                "DXGI duplication returned format {}; BGRA readback requires DXGI_FORMAT_B8G8R8A8_UNORM",
                desc.Format.0
            ),
        });
    }

    let staging_matches = live.staging.is_some()
        && live.staging_desc.is_some_and(|last| {
            last.Width == desc.Width
                && last.Height == desc.Height
                && last.Format == desc.Format
                && last.SampleDesc.Count == desc.SampleDesc.Count
                && last.SampleDesc.Quality == desc.SampleDesc.Quality
        });
    if !staging_matches {
        let mut staging_desc = desc;
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.BindFlags = 0;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        staging_desc.MiscFlags = 0;
        staging_desc.MipLevels = 1;
        staging_desc.ArraySize = 1;
        staging_desc.SampleDesc.Count = 1;
        staging_desc.SampleDesc.Quality = 0;

        let mut staging = None;
        unsafe {
            live.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
        }
        .map_err(|e| Error::BackendUnavailable {
            detail: format!("CreateTexture2D staging readback failed: {e}"),
        })?;
        live.staging = Some(staging.ok_or_else(|| Error::BackendUnavailable {
            detail: "CreateTexture2D returned no staging texture".into(),
        })?);
        live.staging_desc = Some(desc);
    }

    let staging = live
        .staging
        .as_ref()
        .ok_or_else(|| Error::BackendUnavailable {
            detail: "DXGI staging texture is not held".into(),
        })?;
    unsafe {
        live.context.CopyResource(staging, &texture);
    }

    let row_bytes_u32 = desc
        .Width
        .checked_mul(4)
        .ok_or_else(|| Error::BackendUnavailable {
            detail: "DXGI BGRA row size overflow".into(),
        })?;
    let row_bytes = usize::try_from(row_bytes_u32).map_err(|_| Error::BackendUnavailable {
        detail: "DXGI BGRA row size does not fit this process".into(),
    })?;
    let height = usize::try_from(desc.Height).map_err(|_| Error::BackendUnavailable {
        detail: "DXGI frame height does not fit this process".into(),
    })?;
    let pixel_len = row_bytes
        .checked_mul(height)
        .ok_or_else(|| Error::BackendUnavailable {
            detail: "DXGI BGRA frame size overflow".into(),
        })?;

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        live.context
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
    }
    .map_err(|e| Error::BackendUnavailable {
        detail: format!("D3D11 staging texture Map(READ) failed: {e}"),
    })?;
    let _mapped_guard = MappedTextureGuard {
        context: &live.context,
        texture: staging,
    };

    if mapped.pData.is_null() || mapped.RowPitch < row_bytes_u32 {
        return Err(Error::BackendUnavailable {
            detail: format!(
                "D3D11 Map returned invalid BGRA layout (row pitch {}, required {row_bytes_u32})",
                mapped.RowPitch
            ),
        });
    }
    let source_pitch = usize::try_from(mapped.RowPitch).map_err(|_| Error::BackendUnavailable {
        detail: "D3D11 mapped row pitch does not fit this process".into(),
    })?;
    let mut pixels = vec![0u8; pixel_len];
    let source = mapped.pData.cast::<u8>();
    for row in 0..height {
        unsafe {
            std::ptr::copy_nonoverlapping(
                source.add(row * source_pitch),
                pixels.as_mut_ptr().add(row * row_bytes),
                row_bytes,
            );
        }
    }

    Ok(Some(splitdesk_protocol::MediaFrame {
        width: desc.Width,
        height: desc.Height,
        stride: row_bytes_u32,
        timestamp_ns: server_timestamp_ns(),
        cpu_copies: 1,
        pixels,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_bgra_without_start_is_backend_unavailable() {
        let mut capture = DxgiDuplicationCapture::new();
        let err = capture.capture_bgra().unwrap_err();
        assert!(matches!(err, Error::BackendUnavailable { .. }));
    }
}
