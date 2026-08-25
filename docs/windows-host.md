# Windows host

`splitdesk-host-windows` implements `SessionBackend` on Windows. On non-Windows targets the crate still compiles: live spawn returns `Error::BackendUnavailable { detail }`.

## SKU policy

| Host | `SessionSupport` | Interactive sessions SplitDesk will create |
| --- | --- | --- |
| Windows 10 / 11 (workstation) | `WindowsSingleInteractive` | **One**. A second `create` returns `MultiUserNotSupportedByHostOs`. |
| Windows Server with licensed RDS | `WindowsServerRds` | Only via real RDS, never via `termsrv` patches. |

`detect_host_os()` returns `HostOs::Windows`. `Capabilities.multi_user` is false on Windows 10/11. `Capabilities.rds` is true only when the host is actually an RDS-capable Server SKU with RDS present — not because a DLL was patched.

**Never bypass RDS licensing.** No `termsrv.dll` edits, no RDP Wrapper, no concurrent-session cracks, no “undocumented” `WTSCreateSession` shims. If the OS refuses a second interactive session, SplitDesk returns `MultiUserNotSupportedByHostOs` and stops.

## Create

1. If workstation SKU and an interactive session already exists → `MultiUserNotSupportedByHostOs`.
2. Resolve the user. Unknown → `UserNotFound`.
3. Record `windows_session_id: Some(id)` for that session. A different user must not receive the same id (`IsolationViolation`).
4. Capture: `Dxgi` or `WindowsGraphicsCapture` according to probe, else `Unavailable`.
5. Encode: NVENC H.264 first; else `Qsv` / `Amf` if present; else `SoftwareFallback` (CPU-copy warning + measured copy latency); else `Unavailable`.
6. Compositor kind: `WindowsDwm`. SplitDesk does not replace DWM and does not run a compositor as SYSTEM for the user’s desktop.

Status flow is the same as Linux: `Starting` → `Running` → `Connected` | `Detached`.

Disconnect detaches. It does not log the user out and does not destroy the session.

## Input

Inject into **that** Windows session only. No global synthetic driver as the primary path. No SendInput into another user’s session. Cross-session inject is `IsolationViolation` / `PermissionDenied`.

On Disconnect, release every virtual key and button.

## Service vs interactive

A Windows service (`splitdeskd`) running as SYSTEM is a different trust boundary from the interactive user session ([security.md](security.md)). Session 0 isolation applies: the service does not treat Session 0 as an interactive desktop to capture or to inject into.

Token file: `%LOCALAPPDATA%\SplitDesk\daemon.token`, owner-only ACL. If the daemon runs as SYSTEM, that path is SYSTEM’s profile, and the CLI must run in a context that can read it (typically a local named pipe / same-user helper — not a world-readable token).

Default bind remains `127.0.0.1:9823`.

## Capture notes

- DXGI Desktop Duplication and Windows Graphics Capture both need a real display/session. Headless Server Core without a video adapter reports `CaptureKind::Unavailable`.
- Do not fake a GPU adapter in tests. GPU tests are `#[ignore = "requires-gpu"]`.
- `MemoryPath` on this backend typically involves `D3D11` / `D3D12` / `CudaMemory` / `SystemMemory`. Report what is used. Do not claim zero-copy.
