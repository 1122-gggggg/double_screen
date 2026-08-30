# Security

SplitDesk runs other people’s keyboards and a live picture of their desktop. Treat every boundary below as hostile to the others. This is not a zero-copy paper; copies and shared handles are in-scope for leakage.

## Trust boundaries

### 1. Privileged daemon (`splitdeskd`)

May run as root (Linux) or SYSTEM (Windows) so it can spawn a session as another UID / talk to the session manager. It is the most valuable process on the host.

- Control and media binds are loopback-only (`127.0.0.1:9823` and `127.0.0.1:9824` by default). Non-loopback configuration is rejected.
- Token generated at start, 32 random bytes hex. Unix writes use atomic replacement, reject symlink destinations, and keep mode `0600`. Windows uses `MoveFileExW` replace/write-through semantics and inherits the token directory ACL. Never hard-coded.
- Daemon starts **itself**, not Weston/DWM. Compositor as root is a vulnerability, not a feature.
- JSON-lines parser must not trust length or type fields from an unauthenticated peer. No token → `Auth`, no session work.
- Each listener admits at most 64 concurrent connections. JSON lines are limited to 8 KiB before
  parsing; media hello and idle control reads have deadlines to bound slow-client resource use.

### 2. User session

Weston (Linux) or the interactive Windows session runs as the session user. Apps, files, and GPU contexts in that session are that user’s.

- `WAYLAND_DISPLAY` / `windows_session_id` are isolation keys. Different users sharing either is `IsolationViolation`.
- Memory limits and CPU affinity are applied to the user job, not by running the compositor as root.
- The daemon may not pass its root/SYSTEM credentials into the session environment.

### 3. Network client

The client is outside the host. Hello/HelloAck, input, clipboard, and encoded frames cross this boundary.

- v1 is not a public internet service. Local attach and a two-port SSH tunnel are the supported paths.
- `AuthToken` is bound to `{ user, session_id, exp }`. Replay after expiry is `Auth`.
- Client-supplied motion/keys are data, not shell.

### 4. Streamer (capture + encode)

Reads frames from the session’s GPU/compositor and writes compressed video.

- Capture must be the **session’s** output (PipeWire for that `WAYLAND_DISPLAY`, DXGI/WGC for that `windows_session_id`). Capturing seat0 or another user’s desktop is frame leakage.
- Shared GPU handles (`DmaBuf`, `CudaMemory`, `D3D11`, `D3D12`) are capabilities, not a promise that no other process can import them. Restrict handle passing to the encoder process for that session.
- `SoftwareFallback` copies through `SystemMemory`. Those copies are CPU-visible; treat process memory and swap as sensitive.

### 5. Input injection

Writes keys and buttons into the session.

- Primary path is the session seat, not global `uinput`, not `xdotool`, not SendInput to a random HWND.
- Reliable keys/buttons; motion latest-wins. On Disconnect / Detach, **release every held virtual key and button**.
- Cross-user inject is `IsolationViolation` / `PermissionDenied`.

### 6. Filesystem

Tokens, sockets, logs, runtime dirs.

- Unix uses mode `0600` files and `0700` dirs. No `chmod 777`. No world-writable `/tmp/splitdesk`.
- Windows token files inherit the per-user `%LOCALAPPDATA%` directory ACL. Explicit owner-only ACL hardening remains pending and must not be claimed as complete.
- Logs: never clipboard contents, never token values, never frame payloads.
- Session files stay on the host; the protocol does not export `$HOME`.

### 7. Windows service

Session 0 ≠ interactive desktop. A SYSTEM service must not capture or inject Session 0 as if it were the user. Token path under SYSTEM’s profile is not a shared ACL.

### 8. RDS

Windows 10/11: one interactive session. Windows Server RDS only when licensed and real. `termsrv` patches, RDP Wrapper, and concurrent-session cracks are privilege/license bypasses. SplitDesk returns `MultiUserNotSupportedByHostOs` instead.

## Threats

### Session hijack

**Attack:** steal token, guess bind, attach to `sd-001` as someone else.

**Mitigations:** localhost default; 32-byte random token; bind `{ user, session_id, exp }`; `0600` token file; reject mismatched attach as `Auth`. Do not put the token on the media UDP path in clear logs.

### Cross-user input

**Attack:** client A’s keys appear in user B’s Weston/WinSta.

**Mitigations:** isolation on `wayland_display` / `windows_session_id`; input backend keyed by session; no global uinput as primary; PermissionDenied on mismatch.

### Frame leakage

**Attack:** encoder, debug sink, or another session imports the wrong dmabuf/DXGI handle; logs write raw frames; capture grabs seat0.

**Mitigations:** capture bound to the session display; no seat0 master; diagnostics print `MemoryPath` types and `cpu_copies_per_frame`, not pixels; no VNC sidecar.

### Credential / token theft

**Attack:** read `daemon.token`, clipboard of passwords, hard-coded test token left in the binary.

**Mitigations:** generate at start; `zeroize`; never hard-code; clipboard UTF-8 not logged; Unix mode `0600`; Windows per-user directory ACL inheritance; no plaintext password DB.

### Privilege escalation

**Attack:** daemon runs Weston as root; user-controlled `CreateSessionRequest` becomes root command line; Windows service impersonation; RDS crack to get a second SYSTEM-adjacent desktop.

**Mitigations:** compositor always session UID (`systemd-run --uid` when daemon is root; else current user only); no compositor in the systemd `ExecStart`; no termsrv hacks; parse `user` as a username lookup, not as a shell snippet; `memory_limit` / `cpu_affinity` as cgroup fields, not `sh -c`.

## Explicit non-claims

- Not zero-copy. `cpu_copies_per_frame` can be > 0.
- Not “safe to bind on `0.0.0.0`”.
- Not an audited sandbox. Isolation is session-id / UID / WinSta, not a VM.
- Not a replacement for disk encryption or host login policy.

## Dependency audit policy

CI runs RustSec against `Cargo.lock` and denies vulnerabilities, yanked crates, unsoundness, and
unmaintained warnings. The single explicit exception is `RUSTSEC-2024-0384`: current `minifb`
references `instant` only under `wasm32`, while SplitDesk publishes native Linux, Windows, and
macOS clients. Native decoder code must remain free of ignored vulnerabilities; OpenH264 is pinned
to patched version 0.8.1 or newer within the 0.8 compatibility line.
