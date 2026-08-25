# Linux host

`splitdesk-host-linux` implements `SessionBackend` on Linux. On non-Linux targets the crate still compiles: live spawn returns `Error::BackendUnavailable { detail }`.

Session support: `LinuxMultiUser`. Compositor: `Weston` (preferred), `Wlroots` optional, never `None` for a live graphical session.

## Create

1. Resolve `CreateSessionRequest.user`. Unknown user → `UserNotFound`. Empty / root-as-session-user is not a shortcut around isolation.
2. Allocate `SessionId` (`sd-NNN`).
3. Set `WAYLAND_DISPLAY=splitdesk-<id>` (example: `splitdesk-sd-001`). This socket name is per session and must not be reused by a different user (`IsolationViolation`).
4. Spawn **Weston as that UID, not as root**.
   - If the daemon is root: `systemd-run --uid=<user> --gid=<group>` (and a private runtime dir owned by that user).
   - If the daemon is unprivileged: only the current user may get a session; anyone else → `PermissionDenied`.
5. Do **not** take seat0 DRM master. Prefer headless Weston / EGLDevice. The login seat’s GPU node stays with the local session.
6. Status `Starting` → `Running` once the Wayland display is up. Capture attaches via PipeWire when available (`CaptureKind::PipeWire`).

Optional request fields `memory_limit` and `cpu_affinity` are applied to the user slice / cgroup when the daemon can write them; they are not applied by running Weston as root.

## Destroy / disconnect

- Disconnect: `Connected` → `Detached`. Weston stays up.
- Destroy: stop Weston and the user session unit. Release `WAYLAND_DISPLAY`. Do not kill unrelated user processes on the host.

## DRM / GPU

| Do | Do not |
| --- | --- |
| EGLDevice or a render node (`/dev/dri/renderD*`) | `drmSetMaster` on seat0 |
| Headless Weston | Weston on the same tty as GDM/SDDM as root |
| NVENC when `nvidia` + `nvenc` probes succeed | Pretend NVENC exists on a CPU box |
| Report `CaptureKind::Unavailable` / `EncoderKind::Unavailable` | Invent dmabuf handles |

`splitdesk diagnostics gpu` and `scripts/diagnostics-gpu.sh` print probe facts. They never create `/dev/nvidia*` or stub NVENC.

## Encoder / capture

Order: **H.264 + NVENC** if `Capabilities.nvenc`. Else VAAPI / QSV / AMF if actually present. Else `SoftwareFallback` with a CPU-copy warning and `cpu_copies_per_frame >= 1`. Else `Unavailable`.

PipeWire is the capture kind for Weston/wlroots screen content. No VNC server in the session. No `xdotool`. Global `uinput` is not the primary input path; input goes to the session’s virtual seat / Wayland seat for that `WAYLAND_DISPLAY` only.

## systemd

The packaged unit starts **`splitdeskd`**, not Weston. See `packaging/linux/splitdesk.service` and `packaging/linux/install.sh`.

If `splitdeskd` runs as root, that is the privileged daemon trust boundary ([security.md](security.md)). Weston still runs as the session UID.

## Runtime files

- Daemon token: `$XDG_RUNTIME_DIR/splitdesk/daemon.token` or `/tmp/splitdesk-$UID/daemon.token`, mode `0600`.
- Session Wayland socket: private to the session user, not `0777`.
- No world-writable `/tmp/splitdesk` directory.

## Xwayland

`Capabilities.xwayland` is true only when this session’s Weston actually has Xwayland. Host X11 (`:0`) is not the session display.
