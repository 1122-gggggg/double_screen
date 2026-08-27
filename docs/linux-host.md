# Linux host

`splitdesk-host-linux` implements the complete Linux session path. On
non-Linux targets the crate still compiles: live spawn returns
`Error::BackendUnavailable { detail }`.

Session support: `LinuxMultiUser`. Compositor: `Weston` (preferred), `Wlroots` optional, never `None` for a live graphical session.

## Create

1. Resolve `CreateSessionRequest.user`. Unknown user → `UserNotFound`. Empty / root-as-session-user is not a shortcut around isolation.
2. Allocate `SessionId` (`sd-NNN`).
3. Set `WAYLAND_DISPLAY=splitdesk-<id>` (example: `splitdesk-sd-001`). This socket name is per session and must not be reused by a different user (`IsolationViolation`).
4. Spawn **Weston as that UID, not as root**.
   - If the daemon is root: `systemd-run --uid=<user> --gid=<group>` using that user's existing owner-only runtime directory.
   - If the daemon is unprivileged: only the current user may get a session; anyone else → `PermissionDenied`.
5. Require one active session per user. This also makes the per-user
   `weston.pipewire` target unambiguous.
6. Start `weston -Bpipewire --renderer=gl`; this is a headless output and does
   **not** take seat0 DRM master. Weston must create both its private Wayland
   socket and the SplitDesk input socket before the session becomes Running.
7. Start one GStreamer worker as the same UID:
   `pipewiresrc → glupload → GLMemory/NV12 → nvh264enc → h264parse → fdsink`.
8. Relay independent H.264 Annex-B IDR access units through SDFR. The native
   client decodes them with bundled OpenH264 and presents BGRA through minifb.

## Required host components

- Weston 13–16 plus its matching `libweston-<major>-dev` package
- PipeWire running in each session user's `/run/user/<uid>` namespace
- GStreamer tools, PipeWire, GL, parser, and nvcodec plug-ins
- NVIDIA driver/device nodes accessible through the user's supplementary
  groups, with a working `nvh264enc`
- Meson, Ninja, a C compiler, and Wayland development headers to build the
  compositor module

Typical Ubuntu 24.04 packages (package names can differ by distribution):

```bash
sudo apt install weston libweston-13-dev libwayland-dev meson ninja-build \
  pipewire pipewire-bin gstreamer1.0-tools gstreamer1.0-pipewire \
  gstreamer1.0-gl gstreamer1.0-plugins-base gstreamer1.0-plugins-bad
```

The user PipeWire socket must already exist before session creation. Enable
lingering when desktops must survive logout, then start PipeWire from that
user's systemd user session:

```bash
sudo loginctl enable-linger alice
# Run while logged in as alice:
systemctl --user enable --now pipewire.service
```

`packaging/linux/install.sh` builds the module against the installed libweston
major and installs it as `/usr/local/lib/splitdesk/splitdesk-input.so`. The
daemon rejects a missing or group/world-writable module, a missing user
PipeWire socket, or any missing GStreamer element instead of creating a fake
Running session.

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

The completed live Linux path requires **H.264 + NVENC**. The GStreamer caps
require PipeWire DMA-BUF; `glupload` imports it to GLMemory because `nvh264enc`
does not accept DMA-BUF directly. If DMA-BUF negotiation fails, the session is
marked failed instead of silently copying through system memory. Diagnostics report
`DmaBuf → DmaBuf → GlMemory → GlMemory`, not a false direct DMA-BUF/NVENC path.

`gop-size=1`, `bframes=0`, `zerolatency=true`, and repeated sequence headers
make every packet independently decodable. This is deliberately bandwidth
heavier than a long GOP so the depth-1 latest-frame queue remains correct.

Input uses `native/weston-input/splitdesk-input.c`, loaded inside each Weston.
The module creates one `weston_seat`, listens on a mode-0600 socket inside that
user's mode-0700 runtime directory, checks peer credentials, translates v1
virtual-key codes to evdev, and calls libweston's compositor-local notify APIs.
It releases all held keys/buttons on disconnect. No VNC server, `xdotool`,
global `uinput`, login-seat injection, or shared cross-user seat is involved.

## systemd

The packaged unit starts **`splitdeskd`**, not Weston. See `packaging/linux/splitdesk.service` and `packaging/linux/install.sh`.

If `splitdeskd` runs as root, that is the privileged daemon trust boundary ([security.md](security.md)). Weston still runs as the session UID.

## Runtime files

- Daemon token: `$XDG_RUNTIME_DIR/splitdesk/daemon.token` or `/tmp/splitdesk-$UID/daemon.token`, mode `0600`.
- Session Wayland socket: private to the session user, not `0777`.
- Session input socket: `/run/user/<uid>/splitdesk/splitdesk-<id>/input.sock`, mode `0600`.
- Per-session Weston config: the same directory, mode `0600`.
- No world-writable `/tmp/splitdesk` directory.

## Hardware acceptance

CPU CI builds the Rust workspace and the Weston module but cannot claim GPU
success. On the NVIDIA host, create sessions for two real users and verify:

```bash
for element in pipewiresrc glupload glcolorconvert nvh264enc h264parse fdsink; do
  gst-inspect-1.0 "$element" >/dev/null || exit 1
done
splitdesk diagnostics gpu
splitdesk session create --user alice
splitdesk session create --user bob
splitdesk session list
splitdesk diagnostics media-path
```

Then attach two native clients, type and move the pointer concurrently, and
confirm each desktop receives only its own input. Destroy both sessions and
confirm their Weston, GStreamer, Wayland, and input sockets are gone. Do not
check the GPU boxes in `tests/mvp-acceptance.md` until this has been exercised
on the target hardware.

## Xwayland

`Capabilities.xwayland` is true only when this session’s Weston actually has Xwayland. Host X11 (`:0`) is not the session display.
