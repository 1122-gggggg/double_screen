# MVP acceptance

Unchecked on purpose. Tick a box only after the behavior was exercised on a real host. CI CPU runners cannot tick GPU boxes.

## Core types

- [ ] `SessionId` displays as sequential `sd-001`, `sd-002`, …
- [ ] `Resolution` defaults to 1920×1080
- [ ] `CreateSessionRequest.fps` defaults to 60
- [ ] `detect_host_os()` returns `Linux` or `Windows` for the host
- [ ] IPC types serialize (serde) on the daemon JSON-lines path

## Session lifecycle

- [ ] Create: `Starting` → `Running`
- [ ] Attach: `Running` or `Detached` → `Connected`
- [ ] Disconnect / detach: `Connected` → `Detached` and the session is **not** destroyed
- [ ] Destroy: session removed; further info → `SessionNotFound`
- [ ] Unknown user → `UserNotFound`
- [ ] Configurable idle timeout exists and does not fire on a live attached client

## Isolation

- [ ] Two different users never share `wayland_display`
- [ ] Two different users never share `windows_session_id`
- [ ] Attempting to share either → `IsolationViolation`

## Linux host

- [ ] User lookup before spawn
- [ ] `WAYLAND_DISPLAY=splitdesk-<id>`
- [ ] Weston UID equals the session user (not root)
- [ ] Daemon as root uses `systemd-run --uid=…` (or equivalent) for that user
- [ ] Daemon as non-root: other users → `PermissionDenied`; current user may create
- [ ] Does not take seat0 DRM master
- [ ] Headless / EGLDevice preferred over grabbing the login seat
- [ ] `splitdesk-host-linux` compiles on Windows; live spawn returns `BackendUnavailable`
- [ ] Two real users have different Weston PIDs, UIDs, Wayland sockets, input sockets, and PipeWire namespaces
- [ ] A second active session for the same user → `IsolationViolation`
- [ ] Weston module injects only into its compositor-local `weston_seat`
- [ ] Dropping a media connection releases every key/button in that virtual seat

## Windows host

- [ ] Windows 10/11: first interactive session may create
- [ ] Windows 10/11: second create → `MultiUserNotSupportedByHostOs`
- [ ] No `termsrv` patch, RDP Wrapper, or RDS-license bypass in the tree
- [ ] `splitdesk-host-windows` compiles on Linux; live spawn returns `BackendUnavailable`
- [ ] Capture kind is `Dxgi`, `WindowsGraphicsCapture`, or `Unavailable` (not faked)

## Protocol

- [ ] `PROTOCOL_VERSION == 1`
- [ ] Hello / HelloAck with `session_id`, `capabilities`, `selected_codec`
- [ ] Control: Status, SessionCreate, SessionDestroy, SessionList, SessionInfo, Metrics, Diagnostics, Attach, Detach
- [ ] Control/reliable: `u32` LE length + UTF-8 JSON
- [ ] `PointerMotion` latest-wins (stale samples dropped)
- [ ] `PointerButton` and `Key` reliable
- [ ] `Scroll` does not grow an unbounded queue
- [ ] `Cursor` shape/hotspot/visible/position
- [ ] `ClipboardUtf8` delivered; contents never appear in logs
- [ ] Disconnect releases all virtual keys and buttons (no stuck key)
- [ ] `AuthToken` is 32 random bytes hex, bound to `{ user, session_id, exp }`
- [ ] No hard-coded production token

## Daemon / CLI

- [ ] Default bind `127.0.0.1:9823`
- [ ] Default is not `0.0.0.0`
- [ ] JSON-lines request/response
- [ ] Token generated at daemon start
- [ ] Linux token path `$XDG_RUNTIME_DIR/splitdesk/daemon.token` or `/tmp/splitdesk-$UID/daemon.token`, mode `0600`
- [ ] Windows token path `%LOCALAPPDATA%\SplitDesk\daemon.token`, owner-only
- [ ] `splitdesk status`
- [ ] `splitdesk session list`
- [ ] `splitdesk session create --user`
- [ ] `splitdesk session destroy`
- [ ] `splitdesk session info`
- [ ] `splitdesk metrics`
- [ ] `splitdesk diagnostics`
- [ ] `splitdesk diagnostics gpu`
- [ ] `splitdesk diagnostics media-path` prints `MemoryPath`

## Media

- [ ] H.264 + NVENC selected when `Capabilities.nvenc` is true
- [ ] `SoftwareFallback` logs a CPU-copy warning
- [ ] `SoftwareFallback` measures copy latency
- [ ] Capture/motion queue depth 1 (latest frame wins)
- [ ] No unbounded media queues
- [ ] `MemoryPath` includes `cpu_copies_per_frame` (not claimed zero-copy)
- [ ] Encoder/capture/input/clipboard/audio traits exist; GPU tests `#[ignore = "requires-gpu"]`
- [ ] Linux live path is `pipewiresrc → GLMemory → nvh264enc`, not a capability-only stub
- [ ] Each relayed H.264 access unit contains AUD + SPS + PPS + IDR
- [ ] Native client decodes an NVENC H.264 packet and displays the expected pixels

## Metrics

- [ ] All 13 stages from `T0ClientInput` through `T12Presented`
- [ ] `ClockDomain` is `ClientLocal` | `ServerLocal` | `NetworkRtt` | `EstimatedOneWay`
- [ ] No client-monotonic minus server-monotonic without an offset
- [ ] Estimated one-way figures are marked estimates
- [ ] `LatencyStats` has mean, median, p95, p99, max, n

## Packaging / CI

- [ ] systemd unit `ExecStart`s `splitdeskd`, not Weston
- [ ] `install.sh` refuses a unit that starts a compositor
- [ ] `install.sh` never `chmod 777`
- [ ] CI: `ubuntu-latest` and `windows-latest`
- [ ] CI: `cargo fmt --check`, clippy, test, build
- [ ] CI does not run `--ignored` GPU tests and does not stub a GPU

## Prohibited shortcuts (must remain absent)

- [ ] No VNC server as the session
- [ ] No `xdotool` as input
- [ ] No global `uinput` as the primary input path
- [ ] No Electron host compositor / client shell
- [ ] No termsrv / RDS cracks
- [ ] No plaintext password database
- [ ] No hard-coded auth tokens
- [ ] No `chmod 777` on tokens, sockets, or runtime dirs
- [ ] No compositor running as root
