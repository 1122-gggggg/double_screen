# Architecture

SplitDesk is a host-side session broker plus a media path. Files stay on the host. Apps run on the host. The client attaches to a session; it does not download a workspace.

Latency over quality. Latest-frame-wins. Queue depth 1 on motion and on captured frames. No unbounded queues on the media path.

This document describes the contract implemented by the workspace. It does not claim zero-copy.

## Processes

```text
                    127.0.0.1:9823  JSON-lines + token
  splitdesk CLI  ------------------------------------->  splitdeskd
                                                            |
                                                            v
                                                      SessionManager
                                                       /          \
                                          host-linux               host-windows
                                          (Weston as UID)          (1 interactive)
                                               |                         |
                                          PipeWire / GBM             DXGI / WGC
                                               |                         |
                                          EncoderBackend (NVENC first, else named fallback)
                                               |
                                          session protocol (length-prefixed JSON + media)
                                               |
                                  SSH tunnel (remote) / loopback (local)
                                               |
                              shared Linux / Windows / macOS native client
```

- **Control plane:** local daemon, JSON-lines, loopback bind only (default `127.0.0.1:9823`).
- **Media listener:** loopback bind only (default `127.0.0.1:9824`). Remote clients forward both
  listeners through SSH until the built-in encrypted transport is implemented.
- **Admission:** each listener caps concurrent connections at 64. JSON-lines are bounded to 8 KiB;
  media hello and idle control reads have deadlines.
- **Session plane:** length-prefixed JSON for control and reliable input; motion may drop stale samples.
- **Media plane:** capture → optional convert → encode → transport → decode → present. Each hop is a metrics stage. Copies are counted, not hidden.

## Crates

| Crate | Boundary |
| --- | --- |
| `splitdesk-core` | IPC-safe types: `SessionId`, `UserName`, `Resolution`, `HostOs`, `SessionStatus`, `CaptureKind`, `EncoderKind`, `Codec`, `CompositorKind`, `SessionSupport`, `Error`, `Capabilities`, `CreateSessionRequest`, `MemoryType`, `MemoryPath`. `detect_host_os()`. |
| `splitdesk-protocol` | `PROTOCOL_VERSION = 1`, Hello/HelloAck, control, input, cursor, clipboard, disconnect, `AuthToken`. |
| `splitdesk-session` | `SessionRecord`, `SessionBackend`, `SessionManager`. Test backend is `cfg(test)` only. |
| `splitdesk-media` | `EncoderBackend`, `CaptureBackend`, `InputBackend`, `ClipboardBackend`, `AudioBackend`. |
| `splitdesk-metrics` | Stages `T0ClientInput` … `T12Presented` (13), `ClockDomain`, `LatencyStats`. |
| `splitdesk-host-linux` / `splitdesk-host-windows` | OS backends. Cross-compile via `cfg`; live spawn off-OS returns `BackendUnavailable`. |
| `splitdeskd` / `splitdesk-cli` | Daemon and CLI. |
| `splitdesk-client` | Canonical Linux / Windows / macOS native client and CLI parsing. |
| `splitdesk-transport` / compatibility `splitdesk-client-*` / `splitdesk-bench` | Wire, legacy launchers, measurement. |

## Session lifecycle

`SessionStatus`: `Starting` → `Running` → `Connected` or `Detached` → `Stopping` / `Failed`.

- `SessionManager` is an in-memory map.
- Disconnect **detaches**. It does not destroy the session.
- Idle timeout is configurable; expiry destroys or stops according to that config, not according to disconnect.
- `SessionId` is a newtype whose `Display` is sequential `sd-001`, `sd-002`, …
- Default resolution `1920x1080`, default fps `60`.

Isolation: two different users must not share `wayland_display` or `windows_session_id`. A violation is `IsolationViolation`.

Windows 10/11: at most one interactive session. A second create returns `MultiUserNotSupportedByHostOs`. There is no code path that opens a second interactive session by patching RDS.

## Media path

Preferred encode: **H.264 + NVENC**.

Other encoder kinds exist as named capabilities, not as silent stand-ins for NVENC:

- `Nvenc`, `Vaapi`, `Qsv`, `Amf`, `SoftwareFallback`, `Unavailable`

Capture kinds: `PipeWire`, `Dxgi`, `WindowsGraphicsCapture`, `Unavailable`.

`SoftwareFallback` must:

1. Log a CPU-copy warning.
2. Measure copy latency into the metrics field used for copies.
3. Set `MemoryPath.cpu_copies_per_frame` to the number of CPU copies actually performed.

`MemoryPath` fields: `render_output`, `capture`, `conversion`, `encoder_input`, `cpu_copies_per_frame`.

`MemoryType`: `DmaBuf`, `GlMemory`, `CudaMemory`, `D3D11`, `D3D12`, `SystemMemory`.

`splitdesk diagnostics media-path` prints `MemoryPath`. If the path used `SystemMemory` or copied, the printout says so. Do not describe a path as zero-copy unless `cpu_copies_per_frame == 0` and the types in use are GPU-native end-to-end — and even then, say the types, not a slogan.

Latest-frame: capture and motion keep one slot. A new frame/sample overwrites the unread one. The encoder never waits on a growing queue.

The async client writer is bounded. If it applies backpressure, unsent input returns to the
bounded input channels; motion and scroll remain latest-wins while keys/buttons preserve order.

The live SDFR path keeps BGRA payloads in reference-counted byte storage. The daemon writes the
fixed header and payload with vectored I/O, while the client splits complete payloads directly
from its receive buffer. These transport operations do not perform additional full-frame user-space
copies; the Windows DXGI staging readback remains the documented `cpu_copies_per_frame = 1` path.

## Metrics and clocks

Thirteen stages, `T0ClientInput` through `T12Presented`.

Clock domains: `ClientLocal`, `ServerLocal`, `NetworkRtt`, `EstimatedOneWay`.

Never subtract a client monotonic timestamp from a server monotonic timestamp without an offset. Any one-way figure that used an estimated offset is `EstimatedOneWay` and must be marked as an estimate. See [benchmarking.md](benchmarking.md).

## Capabilities

`Capabilities` is what HelloAck and `splitdesk status` / diagnostics report:

`host_os`, `multi_user`, `capture`, `encoder`, `codecs`, `compositor`, `session_support`, `nvidia`, `nvenc`, `pipewire`, `wayland`, `xwayland`, `dxgi`, `rds`.

Booleans are probes of the host, not wishes. Missing NVENC means `nvenc: false` and `encoder` is some other `EncoderKind` or `Unavailable`.

## Error model

`Error` values used across IPC:

`MultiUserNotSupportedByHostOs`, `UserNotFound`, `SessionNotFound`, `PermissionDenied`, `BackendUnavailable { detail }`, `IsolationViolation`, `Protocol`, `Io`, `Auth`, `Unsupported`.

## What the daemon does not do

- Does not bind either TCP plane to a non-loopback address. Remote access uses an SSH tunnel.
- Does not start Weston/DWM as root.
- Does not take seat0 DRM master.
- Does not keep a plaintext password database.
- Does not hard-code tokens.
