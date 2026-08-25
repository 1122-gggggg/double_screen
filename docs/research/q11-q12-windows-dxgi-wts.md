# Q11–Q12 — Windows DXGI / WGC capture and WTS session binding

**Scope:** Official Microsoft Learn / Win32 / Windows Server docs only.  
**SplitDesk policy:** latency over quality; keep a D3D surface into the hardware encoder; no CPU bitmap as primary; Windows 10/11 second interactive session → `MultiUserNotSupportedByHostOs` (`MULTIUSER_NOT_SUPPORTED_BY_HOST_OS`). No `termsrv.dll` patch, no RDP Wrapper, no RDS licensing bypass.

| Q | Verdict | One-line |
| --- | --- | --- |
| **11** | **SUPPORTED** (hardware-MFT path) / **PARTIAL** (inbox H.264 + vendor NVENC SDK) | DXGI Desktop Duplication and Windows Graphics Capture both deliver a D3D surface. Media Foundation can wrap that surface (`MFCreateDXGISurfaceBuffer`) and feed a **certified hardware encoder MFT**. The inbox `mfh264enc.dll` encoder is CPU YUV, not D3D. NVENC’s own D3D11 API is NVIDIA, not Microsoft. |
| **12** | **SUPPORTED** (bind agent to an existing WTS session) / **UNSUPPORTED** (second concurrent interactive session on Win10/11 client SKUs) | `WTSEnumerateSessionsEx` + `WTSQuerySessionInformation` + `WTSQueryUserToken` + `CreateProcessAsUser` bind a process to a session. Session 0 is isolated. `VER_SUITE_SINGLEUSERTS` is the OS/licensing boundary: only one interactive session unless RDSH application-server mode. |

Sources retrieved 2026-08-25 from `learn.microsoft.com`.

---

## Q11. DXGI Desktop Duplication / Windows Graphics Capture → D3D surface → hardware encoder

**Verdict: SUPPORTED** for keeping a D3D11 texture from capture into a GPU encoder via Media Foundation DXGI buffers.  
**PARTIAL** if the claim is “Microsoft documents NVENC D3D11 as the encoder” or “inbox H.264 MFT consumes D3D surfaces.”  
**Do not** use `ID3D11DeviceContext::Map` / GDI `GetDC` / `CopyToMemory` as the primary path.

### 11.1 Desktop Duplication returns a DXGI / D3D11 surface (not a CPU bitmap)

URL: https://learn.microsoft.com/windows/win32/direct3ddxgi/desktop-dup-api

> Windows 8 disables standard Windows 2000 Display Driver Model (XDDM) mirror drivers and offers the desktop duplication API instead. The desktop duplication API provides remote access to a desktop image for collaboration scenarios. Apps can use the desktop duplication API to access frame-by-frame updates to the desktop. **Because apps receive updates to the desktop image in a DXGI surface, the apps can use the full power of the GPU to process the image updates.**

URL: https://learn.microsoft.com/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe

```cpp
HRESULT AcquireNextFrame(
  [in]  UINT                    TimeoutInMilliseconds,
  [out] DXGI_OUTDUPL_FRAME_INFO *pFrameInfo,
  [out] IDXGIResource           **ppDesktopResource
);
```

> When **AcquireNextFrame** returns successfully, the calling application can access the desktop image that **AcquireNextFrame** returns in the variable at *ppDesktopResource*.

Microsoft’s own sample then `QueryInterface`s that `IDXGIResource` to `ID3D11Texture2D` (same page, Desktop Duplication Sample):

```c
hr = DesktopResource->QueryInterface(__uuidof(ID3D11Texture2D),
                                     reinterpret_cast<void **>(&AcquiredDesktopImage));
```

**SplitDesk mapping:** `CaptureKind::Dxgi`. Frame resource is `MemoryType::D3D11`. Primary path is GPU (`CopyResource` / video processor BGRA→NV12 on the GPU), not a system-memory bitmap.

### 11.2 Starting duplication: `IDXGIOutput1::DuplicateOutput`

URL: https://learn.microsoft.com/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput

```cpp
HRESULT DuplicateOutput(
  [in]  IUnknown               *pDevice,
  [out] IDXGIOutputDuplication **ppOutputDuplication
);
```

Documented failure modes (do not invent others):

| HRESULT | Meaning (Microsoft) |
| --- | --- |
| `S_OK` | Duplication interface created. |
| `E_ACCESSDENIED` | “the application does not have access privilege to the current desktop image. For example, only an application that runs at LOCAL_SYSTEM can access the secure desktop.” |
| `DXGI_ERROR_UNSUPPORTED` | “the created IDXGIOutputDuplication interface does not support the current desktop mode or scenario. For example, 8bpp and non-DWM desktop modes are not supported.” |
| `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE` | “DXGI reached the limit on the maximum number of concurrent duplication applications (default of four).” |
| `DXGI_ERROR_SESSION_DISCONNECTED` | “DuplicateOutput failed because the session is currently disconnected.” |
| `E_INVALIDARG` | Wrong device/adapter, or “The calling application is already duplicating this desktop output.” |

`AcquireNextFrame` additionally returns `DXGI_ERROR_ACCESS_LOST` on desktop switch / mode change / DWM on-off / full-screen exclusive, and `DXGI_ERROR_WAIT_TIMEOUT` when no new frame arrived.

**Session 0 / disconnected session:** Microsoft does not publish a dedicated “DuplicateOutput fails in session 0” sentence. Combined with Interactive Services + Session isolation (Q12), a Session-0 service has no interactive `WinSta0` desktop of the logged-on user. `DXGI_ERROR_SESSION_DISCONNECTED` is the documented HRESULT when the session is disconnected. Capture **must** run inside the target interactive WTS session (the per-session agent from Q12), not in the daemon’s Session 0.

### 11.3 Windows Graphics Capture also yields a Direct3D surface

URL: https://learn.microsoft.com/windows/apps/develop/media-authoring-processing/screen-capture

> The Windows.Graphics.Capture namespace provides APIs to acquire frames from a display or application window, to create video streams or snapshots…

> Using the **GraphicsCaptureItem**, create a **Direct3D11CaptureFramePool** with your D3D device, a supported pixel format (**DXGI_FORMAT_B8G8R8A8_UNORM**), number of desired frames (which can be any integer), and frame size.

URL: https://learn.microsoft.com/uwp/api/windows.graphics.capture.direct3d11captureframe.surface

> The Direct3D surface on which the frame was drawn.  
> Property Value: `IDirect3DSurface`

Desktop interop (no picker) for a monitor:

URL: https://learn.microsoft.com/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createformonitor

```cpp
HRESULT CreateForMonitor(
  HMONITOR monitor,
  REFIID   riid,
  void     **result
);
```

Minimum: Windows 10 version 1903 (build 18362).

Unwrap WinRT `IDirect3DSurface` → `ID3D11Texture2D`:

URL: https://learn.microsoft.com/windows/win32/api/windows.graphics.directx.direct3d11.interop/nf-windows-graphics-directx-direct3d11-interop-idirect3ddxgiinterfaceaccess-getinterface

```cpp
HRESULT GetInterface(
  [in]  REFIID iid,
  [out] void   **p
);
```

**SplitDesk mapping:** `CaptureKind::WindowsGraphicsCapture`. Same `MemoryType::D3D11` contract as DXGI duplication. WGC is Win10 1803+ (`Direct3D11CaptureFrame`); `CreateForMonitor` is 1903+. Yellow capture border is system UI; do not try to hide it.

WGC still has to execute **in the session that owns the output**. A Session-0 service cannot capture an interactive user’s DWM.

### 11.4 Keeping the D3D surface into Media Foundation (no CPU bitmap)

Wrap an `ID3D11Texture2D` as an MF sample:

URL: https://learn.microsoft.com/windows/win32/api/mfapi/nf-mfapi-mfcreatedxgisurfacebuffer

```cpp
HRESULT MFCreateDXGISurfaceBuffer(
  [in]  REFIID         riid,
  [in]  IUnknown       *punkSurface,
  [in]  UINT           uSubresourceIndex,
  [in]  BOOL           fBottomUpWhenLinear,
  [out] IMFMediaBuffer **ppBuffer
);
```

> Creates a media buffer to manage a Microsoft DirectX Graphics Infrastructure (DXGI) surface.  
> `riid` … must be **IID_ID3D11Texture2D** or **IID_ID3D12Resource**.

The returned buffer supports `IMFDXGIBuffer` / `IMF2DBuffer` — GPU surface, not a system bitmap.

Share the D3D11 device with an MFT:

URL: https://learn.microsoft.com/windows/win32/medfound/supporting-direct3d-11-video-decoding-in-media-foundation

> The DXGI Device Manager is used to share the Direct3D 11 between components. The DXGI Device Manager exposes the **IMFDXGIDeviceManager** interface. The pipeline sets the **IMFDXGIDeviceManager** pointer on the software decoder by sending the **MFT_MESSAGE_SET_D3D_MANAGER** message.

That article is written for **decoders**, but the same `MFT_MESSAGE_SET_D3D_MANAGER` / `IMFDXGIDeviceManager` contract is the documented way to keep D3D11 in an MF pipeline. `MFCreateDXGISurfaceBuffer` is the encoder-input counterpart: pass the capture (or GPU-converted NV12) texture without `Map`.

### 11.5 Hardware encoder vs inbox software H.264

URL: https://learn.microsoft.com/windows/win32/medfound/h-264-video-encoder

Inbox encoder input subtypes (CPU / system-memory YUV — **not** a D3D surface):

> The input media type must have one of the following subtypes:  
> **MFVideoFormat_I420**, **MFVideoFormat_IYUV**, **MFVideoFormat_NV12**, **MFVideoFormat_YUY2**, **MFVideoFormat_YV12**.

Hardware path (same page):

> **If a certified hardware encoder is present, it will generally be used instead of the inbox system encoder** for Media Foundation related scenarios. Certified encoders are required to support a certain set of **ICodecAPI** properties…

URL: https://learn.microsoft.com/windows/win32/medfound/hardware-mfts

> This topic describes how to write a Media Foundation transform (MFT) that acts as a proxy to a **hardware encoder, decoder, or digital signal processor (DSP)**.  
> If two MFTs represent the same physical device, they can exchange data within the hardware—for example, over a hardware bus. **There is no need to copy the data into system memory and then back to the device.**

**NVENC:** Microsoft Learn does **not** document NVIDIA Video Codec SDK (`nvEncodeAPI`, `ID3D11Texture2D` registration). On NVIDIA GPUs the “certified hardware encoder MFT” is typically the vendor MFT that wraps NVENC; talking to NVENC directly is vendor SDK, out of this research set. SplitDesk `EncoderKind::Nvenc` may use that MFT or the vendor SDK, but **must not claim a Microsoft-documented NVENC API**.

**CPU-copy warning (SplitDesk contract):** `EncoderKind::SoftwareFallback` (inbox `mfh264enc.dll` or a staging-texture `Map`) must log a CPU-copy warning and fill `MemoryPath.cpu_copies_per_frame`. That is not the primary path.

### 11.6 D3D11 shared texture handle types (same-GPU, cross-process)

URL: https://learn.microsoft.com/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgiresource1-createsharedhandle

```cpp
HRESULT CreateSharedHandle(
  [in, optional] const SECURITY_ATTRIBUTES *pAttributes,
  [in]           DWORD                     dwAccess,
  [in, optional] LPCWSTR                   lpName,
  [out]          HANDLE                    *pHandle
);
```

> **CreateSharedHandle** only returns the NT handle when you created the resource as shared and specified that it uses NT handles (that is, you set the **D3D11_RESOURCE_MISC_SHARED_NTHANDLE** and **D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX** flags). If you created the resource as shared and specified that it uses NT handles, you must use **CreateSharedHandle** to get a handle for sharing. In this situation, you can’t use the **IDXGIResource::GetSharedHandle** method because it will fail.

Consumer: `ID3D11Device1::OpenSharedResource1` (handle) or `OpenSharedResourceByName` (named). Close with `CloseHandle`.

**Honest limit:** the texture returned by `AcquireNextFrame` / WGC is owned by DXGI/WGC. It is **not** documented as created with `D3D11_RESOURCE_MISC_SHARED_NTHANDLE`. Primary GPU path is: use it on the **same D3D11 device** that was passed to `DuplicateOutput` / `Direct3D11CaptureFramePool.Create`, `CopyResource` (GPU) into an encoder or shareable texture if a second device/process needs it. That GPU copy is not a CPU bitmap path.

### 11.7 Q11 recommended SplitDesk media path (Windows)

```
WinSta0 interactive session (NOT Session 0)
  ├─ CaptureKind::WindowsGraphicsCapture (1903+ CreateForMonitor)
  │     or CaptureKind::Dxgi (IDXGIOutput1::DuplicateOutput)
  ├─ ID3D11Texture2D (BGRA)          MemoryType::D3D11
  ├─ GPU VideoProcessor / D3D blit   BGRA → NV12 (still GPU)
  ├─ MFCreateDXGISurfaceBuffer
  ├─ Hardware H.264/HEVC MFT         EncoderKind::Nvenc | Qsv | Amf
  │     + IMFDXGIDeviceManager
  └─ SoftwareFallback only if no hardware MFT
        log CPU-copy; measure copy latency
```

`MemoryPath` (diagnostics `media-path`):

| Field | Typical hardware path | SoftwareFallback |
| --- | --- | --- |
| `render_output` | `D3D11` (DWM) | `D3D11` |
| `capture` | `D3D11` | `D3D11` |
| `conversion` | `D3D11` | `SystemMemory` |
| `encoder_input` | `D3D11` | `SystemMemory` |
| `cpu_copies_per_frame` | `0` | `≥ 1` |

---

## Q12. Bind a per-session agent to the correct WTS session ID

**Verdict: SUPPORTED** — enumerate sessions, query identity, take the user token, `CreateProcessAsUser` so the child runs in that session’s `WinSta0\Default`.  
**UNSUPPORTED** — creating a **second concurrent interactive session** on Windows 10/11 client SKUs. Return `MULTIUSER_NOT_SUPPORTED_BY_HOST_OS`. Do not patch `termsrv.dll` or use RDP Wrapper.

### 12.1 Session model

URL: https://learn.microsoft.com/windows/win32/termserv/terminal-services-sessions

> When a user logs on to a Remote Desktop Services–enabled computer, a session is started for the user. Each session is identified by a unique session ID.  
> **Each remote desktop session is associated with an interactive window station. The only supported window station name for an interactive window station is "WinSta0"; therefore each session is associated with its own "WinSta0" window station.** There are three standard desktops for each window station: the Winlogon desktop, the screen saver desktop, and the interactive desktop.

Console session ID: `WTSGetActiveConsoleSessionId`  
URL: https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-wtsgetactiveconsolesessionid

> Retrieves the session identifier of the console session. The console session is the session that is currently attached to the physical console. Note that it is not necessary that Remote Desktop Services be running for this function to succeed.

### 12.2 WTS API list (from `wtsapi32.h`)

Header index: https://learn.microsoft.com/windows/win32/api/wtsapi32/

APIs SplitDesk actually needs to **bind** an agent (not a complete dump of the header):

| API | Role |
| --- | --- |
| `WTSOpenServerExW` / `WTS_CURRENT_SERVER_HANDLE` | Local or RDVH server handle. |
| `WTSEnumerateSessionsExW` | List sessions + `WTS_SESSION_INFO_1` (session ID, user, state). Free with `WTSFreeMemoryEx`. |
| `WTSEnumerateSessionsW` | Older list (`WTS_SESSION_INFO`). Free with `WTSFreeMemory`. |
| `WTSQuerySessionInformationW` | Per-session fields (`WTS_INFO_CLASS`). Free with `WTSFreeMemory`. |
| `WTSQueryUserToken` | Primary token for `SessionId`. **LocalSystem + `SE_TCB_NAME` only.** |
| `WTSGetActiveConsoleSessionId` | Physical console session. |
| `ProcessIdToSessionId` | Fallback current-session ID if RDS is not running (`WTSQuerySessionInformation` remarks). |
| `WTSRegisterSessionNotification(Ex)` | `WM_WTSSESSION_CHANGE` (connect/disconnect/logon). |
| `CreateProcessAsUserW` | Spawn agent in the token’s session (`processthreadsapi.h`). |
| `CreateEnvironmentBlock` / `LoadUserProfileW` | User environment + `HKEY_CURRENT_USER` (documented as **not** automatic in `CreateProcessAsUser`). |
| `WTSFreeMemory` / `WTSFreeMemoryExW` | Release WTS buffers. |
| `WTSCloseServer` | Close `WTSOpenServer*` handle. |

Do **not** use as a multi-user desktop substitute: `WTSConnectSession`, `WTSStartRemoteControlSession`, `WTSEnableChildSessions`, `WTSCreateListener`. Child Sessions are a nested-RDP feature, not extra interactive desktops for SplitDesk.

#### `WTSEnumerateSessionsExW`

URL: https://learn.microsoft.com/windows/win32/api/wtsapi32/nf-wtsapi32-wtsenumeratesessionsexw

```cpp
BOOL WTSEnumerateSessionsExW(
  [in]      HANDLE               hServer,
  [in, out] DWORD                *pLevel,
  [in]      DWORD                Filter,
  [out]     PWTS_SESSION_INFO_1W *ppSessionInfo,
  [out]     DWORD                *pCount
);
```

> Always set [*pLevel*] to one. Always set [*Filter*] to zero.  
> To enumerate a session, you need to have the Query Information permission for that session.

#### `WTSQuerySessionInformationW`

URL: https://learn.microsoft.com/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw

```cpp
BOOL WTSQuerySessionInformationW(
  [in]  HANDLE         hServer,
  [in]  DWORD          SessionId,
  [in]  WTS_INFO_CLASS WTSInfoClass,
  [out] LPWSTR         *ppBuffer,
  [out] DWORD          *pBytesReturned
);
```

Useful `WTS_INFO_CLASS` values  
URL: https://learn.microsoft.com/windows/win32/api/wtsapi32/ne-wtsapi32-wts_info_class

| Class | Use |
| --- | --- |
| `WTSSessionId` | Confirm ID. |
| `WTSUserName` / `WTSDomainName` | Bind SplitDesk `UserName` to a session. |
| `WTSConnectState` | `WTSActive` vs disconnected. |
| `WTSWinStationName` | Session name (not the window-station string; each session still has its own `WinSta0`). |
| `WTSClientProtocolType` | `0` = console, `2` = RDP. |
| `WTSIsRemoteSession` | Local vs remote (current server only). |

> To determine whether your application is running on the physical console, you must specify **WTS_CURRENT_SESSION** … and **WTSClientProtocolType**. If *ppBuffer* is "0", the session is attached to the physical console.

#### `WTSQueryUserToken`

URL: https://learn.microsoft.com/windows/win32/api/wtsapi32/nf-wtsapi32-wtsqueryusertoken

```cpp
BOOL WTSQueryUserToken(
  [in]  ULONG   SessionId,
  [out] PHANDLE phToken
);
```

> Obtains the primary access token of the logged-on user specified by the session ID. To call this function successfully, the calling application must be running within the context of the **LocalSystem account** and have the **SE_TCB_NAME** privilege.  
> **Caution** WTSQueryUserToken is intended for highly trusted services. Service providers must use caution that they do not leak user tokens … must close token handles after they have finished using them.  
> Any program running in the context of a service will have a session identifier of zero (0).

Daemon implication: token query lives in the Session-0 service; the **agent** is the process created *as* that token.

#### `CreateProcessAsUserW`

URL: https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw

```cpp
BOOL CreateProcessAsUserW(
  [in, optional]      HANDLE                hToken,
  [in, optional]      LPCWSTR               lpApplicationName,
  [in, out, optional] LPWSTR                lpCommandLine,
  ...
  [in]                LPSTARTUPINFOW        lpStartupInfo,
  [out]               LPPROCESS_INFORMATION lpProcessInformation
);
```

> **Terminal Services:** The process is run in the session specified in the token.  
> **Terminal Services:** You cannot inherit handles across sessions.  
> By default, **CreateProcessAsUser** creates the new process on a noninteractive window station … To enable user interaction with the new process, you must specify … **"winsta0\\default"** in the **lpDesktop** member of the **STARTUPINFO** structure.  
> **CreateProcessAsUser** does not load the specified user's profile into the **HKEY_USERS** registry key. Therefore … **LoadUserProfile** … before calling **CreateProcessAsUser**.

Privileges: caller typically needs `SE_INCREASE_QUOTA_NAME` and may need `SE_ASSIGNPRIMARYTOKEN_NAME`.

**SplitDesk spawn sequence (existing session only):**

1. `WTSEnumerateSessionsExW` → find `WTSActive` session whose `WTSUserName` matches.  
2. If host is Win10/11 client (`VER_NT_WORKSTATION` + `VER_SUITE_SINGLEUSERTS`) and a SplitDesk session already occupies that interactive session, or the caller asked for a **second** interactive desktop: return `MULTIUSER_NOT_SUPPORTED_BY_HOST_OS`.  
3. `WTSQueryUserToken(sessionId)` from LocalSystem.  
4. `LoadUserProfile` + `CreateEnvironmentBlock`.  
5. `STARTUPINFO.lpDesktop = L"winsta0\\default"`; `bInheritHandles = FALSE` (cross-session inherit is documented as invalid).  
6. `CreateProcessAsUserW` → capture/encode/input agent. Store `windows_session_id`.  
7. Isolation: two SplitDesk users must not share `windows_session_id` (core contract).

### 12.3 Session 0 isolation

URL: https://learn.microsoft.com/windows/win32/services/interactive-services

> **Services cannot directly interact with a user as of Windows Vista.**  
> **All services run in Terminal Services session 0.** Therefore, if an interactive service displays a user interface, it is visible only to the user who connected to session 0. Because there is no way to guarantee that the interactive user is connected to session 0, do not configure a service to run as an interactive service…

Documented indirect pattern (this is the SplitDesk daemon → agent model):

> Create a separate hidden GUI application and use the **CreateProcessAsUser** function to run the application within the context of the interactive user. Design the GUI application to communicate with the service through some method of interprocess communication (IPC)…

`SERVICE_INTERACTIVE_PROCESS` is obsolete (`NoInteractiveServices` defaults to 1). Do not use it.

**Capture/input in Session 0:** DXGI `DuplicateOutput` / WGC / `SendInput` against the user’s desktop will not see Session *N*’s DWM. The per-session agent must run in session *N*.

### 12.4 Windows 10/11 vs Windows Server (licensing / OS boundary)

#### Official one-interactive-session flag

URL: https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-osversioninfoexw

`OSVERSIONINFOEXW.wSuiteMask`:

> **VER_SUITE_SINGLEUSERTS** (0x00000100) — **Remote Desktop is supported, but only one interactive session is supported. This value is set unless the system is running in application server mode.**  
> **VER_SUITE_TERMINAL** (0x00000010) — Terminal Services is installed. This value is always set. **If VER_SUITE_TERMINAL is set but VER_SUITE_SINGLEUSERTS is not set, the system is running in application server mode.**  
> **VER_SUITE_MULTIUSERTS** (0x00020000) — AppServer mode is enabled.

`wProductType`:

| Value | Meaning |
| --- | --- |
| `VER_NT_WORKSTATION` (1) | Client OS (Windows 10/11, etc.). |
| `VER_NT_SERVER` (3) | Windows Server (not a DC). |
| `VER_NT_DOMAIN_CONTROLLER` (2) | Server that is a DC. |

Version table on the same page: Windows 10 is `10.0` with `wProductType == VER_NT_WORKSTATION`; Windows Server 2016+ is `10.0` with `wProductType != VER_NT_WORKSTATION`.

**SplitDesk detection (no hacks):**

1. `RtlGetVersion` into `RTL_OSVERSIONINFOEXW` (avoids manifested `GetVersionEx` lying about 6.2).  
   URL: https://learn.microsoft.com/windows-hardware/drivers/ddi/wdm/nf-wdm-rtlgetversion  
   > The **RtlGetVersion** routine returns version information about the currently running operating system.  
2. Workstation vs Server: `wProductType`.  
3. Multi-session RDSH: `VER_SUITE_TERMINAL` set **and** `VER_SUITE_SINGLEUSERTS` **clear** (application server mode), and/or `VER_SUITE_MULTIUSERTS`.  
4. SKU: `GetProductInfo` — URL: https://learn.microsoft.com/windows/win32/api/sysinfoapi/nf-sysinfoapi-getproductinfo  
   Client examples: `PRODUCT_CORE` (Home), `PRODUCT_PROFESSIONAL`, `PRODUCT_ENTERPRISE`, `PRODUCT_PRO_WORKSTATION`.  
   Server examples: `PRODUCT_STANDARD_SERVER`, `PRODUCT_DATACENTER_SERVER`.  
   `PRODUCT_SERVERRDSH` (0xAF) = “Windows 10 Enterprise for Virtual Desktops” (AVD multi-session image, not an on-prem client SKU).

`GetVersionEx` is compatibility-shimmed; Microsoft’s own version article says un-manifested apps see Windows 8 (6.2). Prefer `RtlGetVersion` + `wProductType` / `wSuiteMask` + `GetProductInfo`.

#### Windows Enterprise multi-session is AVD-only

URL: https://learn.microsoft.com/azure/virtual-desktop/windows-multisession-faq

> Windows 10 Enterprise multi-session and Windows 11 Enterprise multi-session, allows multiple concurrent interactive sessions. Previously, only Windows Server could do this.  
> **We don't allow customers to run Windows Enterprise multi-session in production environments outside of the Azure Virtual Desktop service.** … **It's against the licensing agreement to run Windows multi-session outside of the Azure Virtual Desktop service for production purposes.**

IT can use existing per-user Windows licensing **instead of RDS CALs** *inside AVD*. That does **not** turn a retail Win10/11 PC into a multi-session host.

#### RDS CALs (Windows Server session hosts)

URL: https://learn.microsoft.com/windows-server/remote/remote-desktop-services/rds-client-access-license

> **Each user and device that connects to a Remote Desktop Services session host or Azure Virtual Desktop session host running Windows Server needs a Remote Desktop Services (RDS) client access license (CAL).**  
> You use a Remote Desktop Licensing server to install, issue, and track RDS CALs.  
> There are two types of RDS CALs: **per device** and **per user**.  
> There's a licensing grace period of 120 days during which no license server is required. Once the grace period ends, clients must have a valid RDS CAL issued by a license server before they can sign in a remote session.

Honest SplitDesk reporting:

| Host | `session_support` | Extra interactive session | RDS CAL |
| --- | --- | --- | --- |
| Windows 10/11 Home/Pro/Enterprise (workstation, `VER_SUITE_SINGLEUSERTS`) | `WindowsSingleInteractive` | **No.** Second create → `MULTIUSER_NOT_SUPPORTED_BY_HOST_OS`. | Not applicable. Client SKUs are not RDSH. |
| Windows Enterprise multi-session | AVD-only | Not an on-prem SplitDesk host. | AVD Windows licensing, not “install RDS CALs on a PC”. |
| Windows Server **without** RDSH (admin Remote Desktop) | still not a general multi-user desktop product | Two admin connections are an **administrative** RDS feature, not a SplitDesk session factory. | Admin access is not a substitute for RDS CALs if you host extra users. |
| Windows Server **with** RD Session Host (application server mode) | `WindowsServerRds` | Multiple sessions are an OS role, **licensed**. | **Yes — each user/device needs an RDS CAL** (quote above). SplitDesk must say so; it does not issue CALs. |

### 12.5 What SplitDesk must not do

- Patch or replace `termsrv.dll`.  
- RDP Wrapper / “enable multiple RDP sessions” registry hacks.  
- Treat Fast User Switching disconnected sessions as extra **concurrent interactive** SplitDesk desktops on Win10/11.  
- Run capture/input in Session 0 and pretend it is the user’s desktop.  
- Use `SERVICE_INTERACTIVE_PROCESS`.  
- Ship Windows Enterprise multi-session outside Azure Virtual Desktop.  
- Imply that buying RDS CALs unlocks multi-session on Windows 10/11 client editions.

There is **no** Microsoft-documented supported bypass of the one-interactive-session client-OS boundary. SplitDesk returns `MULTIUSER_NOT_SUPPORTED_BY_HOST_OS` and stops.

---

## API cheat sheet (exact signatures)

```cpp
// dxgi1_2.h
HRESULT IDXGIOutput1::DuplicateOutput(IUnknown *pDevice, IDXGIOutputDuplication **ppOutputDuplication);
HRESULT IDXGIOutputDuplication::AcquireNextFrame(UINT TimeoutInMilliseconds, DXGI_OUTDUPL_FRAME_INFO *pFrameInfo, IDXGIResource **ppDesktopResource);
HRESULT IDXGIResource1::CreateSharedHandle(const SECURITY_ATTRIBUTES *pAttributes, DWORD dwAccess, LPCWSTR lpName, HANDLE *pHandle);

// mfapi.h
HRESULT MFCreateDXGISurfaceBuffer(REFIID riid, IUnknown *punkSurface, UINT uSubresourceIndex, BOOL fBottomUpWhenLinear, IMFMediaBuffer **ppBuffer);

// windows.graphics.capture.interop.h
HRESULT IGraphicsCaptureItemInterop::CreateForMonitor(HMONITOR monitor, REFIID riid, void **result);

// wtsapi32.h
BOOL WTSEnumerateSessionsExW(HANDLE hServer, DWORD *pLevel, DWORD Filter, PWTS_SESSION_INFO_1W *ppSessionInfo, DWORD *pCount);
BOOL WTSQuerySessionInformationW(HANDLE hServer, DWORD SessionId, WTS_INFO_CLASS WTSInfoClass, LPWSTR *ppBuffer, DWORD *pBytesReturned);
BOOL WTSQueryUserToken(ULONG SessionId, PHANDLE phToken);
DWORD WTSGetActiveConsoleSessionId(void);

// processthreadsapi.h
BOOL CreateProcessAsUserW(HANDLE hToken, LPCWSTR lpApplicationName, LPWSTR lpCommandLine, ... LPSTARTUPINFOW lpStartupInfo, LPPROCESS_INFORMATION lpProcessInformation);

// winnt.h / ntdll
NTSTATUS RtlGetVersion(PRTL_OSVERSIONINFOW lpVersionInformation);
BOOL GetProductInfo(DWORD dwOSMajorVersion, DWORD dwOSMinorVersion, DWORD dwSpMajorVersion, DWORD dwSpMinorVersion, PDWORD pdwReturnedProductType);
```

---

## Source index

| URL | Used for |
| --- | --- |
| https://learn.microsoft.com/windows/win32/direct3ddxgi/desktop-dup-api | DXGI surface, GPU processing, QI to `ID3D11Texture2D` |
| https://learn.microsoft.com/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe | `AcquireNextFrame` |
| https://learn.microsoft.com/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutput1-duplicateoutput | `DuplicateOutput` errors, 4-app cap, session disconnected |
| https://learn.microsoft.com/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgiresource1-createsharedhandle | NT shared handles vs `GetSharedHandle` |
| https://learn.microsoft.com/windows/apps/develop/media-authoring-processing/screen-capture | WGC frame pool |
| https://learn.microsoft.com/uwp/api/windows.graphics.capture.direct3d11captureframe.surface | `IDirect3DSurface` |
| https://learn.microsoft.com/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createformonitor | Silent monitor item |
| https://learn.microsoft.com/windows/win32/api/windows.graphics.directx.direct3d11.interop/nf-windows-graphics-directx-direct3d11-interop-idirect3ddxgiinterfaceaccess-getinterface | Surface → D3D11 |
| https://learn.microsoft.com/windows/win32/api/mfapi/nf-mfapi-mfcreatedxgisurfacebuffer | DXGI media buffer |
| https://learn.microsoft.com/windows/win32/medfound/supporting-direct3d-11-video-decoding-in-media-foundation | `IMFDXGIDeviceManager` |
| https://learn.microsoft.com/windows/win32/medfound/h-264-video-encoder | Inbox YUV vs certified hardware encoder |
| https://learn.microsoft.com/windows/win32/medfound/hardware-mfts | Hardware MFT, no system-memory copy |
| https://learn.microsoft.com/windows/win32/api/wtsapi32/ | `wtsapi32.h` index |
| https://learn.microsoft.com/windows/win32/api/wtsapi32/nf-wtsapi32-wtsenumeratesessionsexw | Enumerate |
| https://learn.microsoft.com/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw | Query |
| https://learn.microsoft.com/windows/win32/api/wtsapi32/ne-wtsapi32-wts_info_class | Info classes |
| https://learn.microsoft.com/windows/win32/api/wtsapi32/nf-wtsapi32-wtsqueryusertoken | User token |
| https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw | Spawn in token session |
| https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-wtsgetactiveconsolesessionid | Console session |
| https://learn.microsoft.com/windows/win32/termserv/terminal-services-sessions | Per-session `WinSta0` |
| https://learn.microsoft.com/windows/win32/services/interactive-services | Session 0 isolation |
| https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-osversioninfoexw | `VER_SUITE_SINGLEUSERTS` / `VER_NT_*` |
| https://learn.microsoft.com/windows-hardware/drivers/ddi/wdm/nf-wdm-rtlgetversion | `RtlGetVersion` |
| https://learn.microsoft.com/windows/win32/api/sysinfoapi/nf-sysinfoapi-getproductinfo | SKU |
| https://learn.microsoft.com/windows-server/remote/remote-desktop-services/rds-client-access-license | RDS CAL |
| https://learn.microsoft.com/azure/virtual-desktop/windows-multisession-faq | Multi-session AVD-only |
