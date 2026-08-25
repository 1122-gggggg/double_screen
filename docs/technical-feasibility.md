# Technical feasibility (Phase 0)

Status: compiled from the four Phase 0 research notes. No extra
experiments. No CloudCompare run. No cargo, no tests, no marketing
PASS.

Sources (do not treat this file as a substitute for them):

- [docs/research/q01-q04-weston-drm-nvidia.md](research/q01-q04-weston-drm-nvidia.md)
- [docs/research/q05-q07-pipewire-nvenc.md](research/q05-q07-pipewire-nvenc.md)
- [docs/research/q08-q10-libei-xwayland.md](research/q08-q10-libei-xwayland.md)
- [docs/research/q11-q12-windows-dxgi-wts.md](research/q11-q12-windows-dxgi-wts.md)

Versions those notes actually checked: Weston 16.0.90, PipeWire 1.6.8,
GStreamer nvcodec / `nvh264enc` published docs, NVIDIA Video Codec
SDK 13.0, NVIDIA Linux desktop README **580.142** (Q1–Q4) and
**570.133.07** (Q8–Q10 Xwayland/GBM chapters), NVIDIA Jetson r36.5.2,
libei/libeis 1.6.0, xorg/xserver 26.1.99.1, Qt 6.11.2, CloudCompare
`master` BUILD.md, Microsoft Learn retrieved 2026-08-25.

Verdicts are only `SUPPORTED`, `PARTIAL`, `UNSUPPORTED`, or
`UNKNOWN`. A hop that exists on one side of a pipe is not a
zero-copy path.

## Honesty (non-negotiable)

These four facts are **not** supported as Phase 0 product claims:

1. **DMA-BUF into `nvh264enc` is not supported.** The official sink
   pad template lists `CUDAMemory`, `D3D12Memory`, `GLMemory`, and
   system `video/x-raw`. `memory:DMABuf` is absent. A PipeWire
   `SPA_DATA_DmaBuf` that lands in GStreamer is still
   `MemoryType::DmaBuf`. It must be imported into `GLMemory` or
   `CUDAMemory` (or copied to system memory) before encode.
2. **CloudCompare was not tested.** Q10 is `UNKNOWN`. Architectural
   Qt/X11/Wayland prerequisites are not a run result.
3. **NVIDIA desktop README 580.142 still says GBM KMS submit is
   unsupported.** Quote, do not resolve, the contradiction with the
   same README’s GBM compositor chapter. Do not use Weston `-Bdrm`
   on NVIDIA as the multi-session path.
4. **Inbox `mfh264enc.dll` is a CPU YUV encoder.** It consumes
   system-memory subtypes (`MFVideoFormat_I420` / `IYUV` / `NV12` /
   `YUY2` / `YV12`), not a D3D surface. Hardware encode on Windows
   is a certified hardware MFT (or a vendor NVENC SDK, which
   Microsoft Learn does not document).

Do not describe any path as zero-copy unless
`MemoryPath.cpu_copies_per_frame == 0` **and** every printed
`MemoryType` is GPU-native end-to-end — and even then print the
types, not a slogan.

## Verdict table

| Q | Topic | Verdict |
| --- | --- | --- |
| 1 | Weston PipeWire backend + GPU renderer | **SUPPORTED** |
| 2 | Independent Weston per Linux user (`--socket`, `XDG_RUNTIME_DIR`) | **SUPPORTED** |
| 3 | Avoid competing for physical DRM master | **SUPPORTED** |
| 4 | NVIDIA headless OpenGL / Vulkan | **PARTIAL** |
| 5 | PipeWire frames stay DMA-BUF / GPU-resident into GStreamer | **PARTIAL** |
| 6 | GStreamer `nvh264enc` GPU memory types | **PARTIAL** (`CUDAMemory`/`GLMemory` yes; `DMABuf` **UNSUPPORTED**) |
| 7 | Per-session Weston input (not uinput / xdotool) | **PARTIAL** |
| 8 | libei / libeis placement | **PARTIAL** |
| 9 | XWayland GPU accel under per-user Weston / headless | **PARTIAL** |
| 10 | CloudCompare (Qt + OpenGL, often X11) | **UNKNOWN** |
| 11 | DXGI / WGC → D3D surface → hardware encoder | **SUPPORTED** (hardware MFT) / **PARTIAL** (inbox H.264 + vendor NVENC SDK) |
| 12 | Bind agent to WTS session | **SUPPORTED** (existing session) / **UNSUPPORTED** (second interactive on Win10/11) |

---

## Q1. Can the Weston PipeWire backend run together with a GPU renderer?

**Verdict: SUPPORTED**

Backend and renderer are separate libweston objects. PipeWire is an
output path (one PipeWire node per output). Composition is a
renderer (`gl`, `vulkan`, or `pixman`). They run in one Weston
process. The PipeWire backend initializes GL on the Mesa
**surfaceless** EGL platform, not DRM/GBM. Output enablement for GL
uses an FBO, not a KMS plane.

### Official upstream evidence

<https://wayland.pages.freedesktop.org/weston/toc/running-weston.html>
(Weston 16.0.90 *Running Weston*):

> Available back-ends: **pipewire** — run without input, output into
> a PipeWire node. … The job of gathering all the surfaces … is
> performed by a *renderer*. … There are OpenGL ES and Vulkan
> renderers, which will often be accelerated by your GPU when
> suitable drivers are installed. … You can select between these
> with the `--renderer=gl`, `--renderer=vulkan` and
> `--renderer=pixman` arguments when starting Weston.

`weston(1)`:

> **pipewire** — The PipeWire backend runs in memory without the
> need of graphical hardware and creates a PipeWire node for each
> output. It can be used to capture Weston outputs for processing
> with another application.

Upstream `libweston/backend-pipewire/pipewire.c` (Weston master,
cited in the Q1–Q4 note) sets
`.egl_platform = EGL_PLATFORM_SURFACELESS_MESA` for
`WESTON_RENDERER_GL`. `WESTON_RENDERER_AUTO` on this backend falls
through to **Pixman** (CPU).

### Implication for SplitDesk

- One Weston process per session: `-Bpipewire --renderer=gl`.
- PipeWire is `CaptureKind::PipeWire`. It is not a second compositor.
- **Do not** also load `-Bdrm`. That is only for a local monitor and
  takes DRM master on the primary.
- `--renderer=gl` is mandatory. Omitting it yields a software
  compositor.
- DMA-BUF out of Weston is best-effort linear only (upstream TODO:
  modifier negotiation). Treat `SPA_DATA_MemFd` as the expected
  fallback. This is **not** a zero-copy-into-NVENC claim (see Q5–Q6).
- Require Weston ≥13 (GL on pipewire); prefer 14/15/16 matching
  current docs.

---

## Q2. Can each Linux user start an independent Weston instance (socket, `XDG_RUNTIME_DIR`)?

**Verdict: SUPPORTED**

Weston is a per-process compositor. Isolation is the Wayland socket
path `$XDG_RUNTIME_DIR/<socket-name>`. systemd already gives each
logged-in user a private runtime dir. Multiple Westons for the
**same** UID need distinct `--socket` names.

### Official upstream evidence

<https://wayland.pages.freedesktop.org/weston/toc/running-weston.html>:

> Weston creates its unix socket file (for example, wayland-1) in
> the directory specified by the required environment variable
> `$XDG_RUNTIME_DIR`. Clients use the same variable to find that
> socket. Normally this should already be provided by systemd.

`weston(1)` ENVIRONMENT / `--socket`:

> **XDG_RUNTIME_DIR** — The directory for Weston's socket and lock
> files. … `-S name, --socket=name` — Weston will listen in the
> Wayland socket called *name*. Weston will export `WAYLAND_DISPLAY`
> with this value in the environment for all child processes.

Upstream `frontend/main.c` `verify_xdg_runtime_dir()`: missing
`XDG_RUNTIME_DIR`, not a directory, or (warning) mode/owner not
`0700`/`getuid()` is fatal or unsafe. Socket names are not
authenticated; isolation is directory permissions.

### Implication for SplitDesk

Per session, as the **session UID** (never root):

| item | value |
| --- | --- |
| `XDG_RUNTIME_DIR` | `/run/user/<uid>` if a user session exists; else a dedicated dir `0700` owned by that UID (not a shared `/tmp`) |
| `--socket` / `WAYLAND_DISPLAY` | `splitdesk-<SessionId>` (e.g. `splitdesk-sd-001`) |
| compositor UID | the session user; `systemd-run --uid=…` only when the daemon is root |

Two users may both use the socket name `splitdesk-sd-001` because
the directories differ. Two sessions for the **same** user must not
share a socket name. Different users sharing `wayland_display` is
`IsolationViolation`.

Always pass `-Bpipewire` (or `-Bheadless`) and **unset inherited
`WAYLAND_DISPLAY`**. If `WAYLAND_DISPLAY` is set and `-B` is
omitted, Weston nests. If `DISPLAY` is set, `WAYLAND_DISPLAY` is
unset, and `-B` is omitted, the default backend is `x11`.

A Weston PipeWire backend started as user U talks to U’s PipeWire,
not another user’s. Phase 0 must start or reuse that user’s PipeWire
daemon; a fake runtime dir does not create one.

---

## Q3. How to avoid every session competing for physical DRM master?

**Verdict: SUPPORTED**

Do not use the DRM/KMS backend. Use PipeWire or headless. GPU access
then goes through a **render node** (`/dev/dri/renderD*`), which the
kernel documents as having **no DRM-Master**. Physical `card*`
master stays with the seat0 compositor (or unused).

### Official upstream evidence

`weston(1)`: pipewire and headless “run in memory” / “without the
need of graphical hardware.”

`weston-drm(7)` on the backend that **does** take the device:

> The DRM backend is the native Weston backend for systems that
> support the Linux kernel DRM, kernel mode setting (KMS), and evdev
> input devices. … It also relies on the Mesa GBM interface. … The
> backend uses the Linux KMS API to detect connected monitors. …
> The backend chooses the DRM graphics device first based on seat
> id.

Linux DRM uAPI, *Render nodes*
<https://docs.kernel.org/gpu/drm-uapi.html>:

> Render nodes solely serve render clients, that is, no modesetting
> or privileged ioctls can be issued on render nodes. … render
> nodes also drop the DRM-Master concept. There is no reason to
> associate render clients with a DRM-Master as they are independent
> of any graphics server. Besides, they must work without any
> running master, anyway.

Weston headless GL init uses the same
`EGL_PLATFORM_SURFACELESS_MESA` / `egl_native_display = NULL` as
PipeWire — no `card*` fd. The DRM backend is
`EGL_PLATFORM_GBM_KHR`.

### Implication for SplitDesk

| approach | DRM master? | Phase 0 |
| --- | --- | --- |
| `-Bpipewire --renderer=gl` | no | **default** |
| `-Bheadless --renderer=gl` | no | fallback if PipeWire is absent |
| `-Bdrm` / `-Bdrm,pipewire` | **yes** | forbidden for extra sessions |
| extra GPU + `-Bdrm --seat=…` | yes, on that card | only if a whole unused GPU is dedicated |

Session processes need `render` group / ACL on
`/dev/dri/renderD*`. They must not be granted master on
`/dev/dri/card*`. Do not open `card0` from a session Weston.
`nvidia-drm.modeset=1` is **not** a way to share master; leave
modeset to the seat compositor.

Headless with `WESTON_RENDERER_AUTO` becomes the **noop** renderer.
Always pass `--renderer=gl`. Many concurrent GL compositors share
one GPU; kernel render nodes allow it, they do not guarantee
latency isolation. Surface contention in `diagnostics gpu`.

---

## Q4. How does NVIDIA provide headless OpenGL / Vulkan?

**Verdict: PARTIAL**

NVIDIA documents several real headless/bootstrap paths. The platform
Weston PipeWire/headless actually request
(`EGL_MESA_platform_surfaceless`) is listed for **Jetson** EGL, not
in the **desktop** Linux README. Desktop README 580.142 still
carries a stale “GBM+KMS not supported” line next to a whole chapter
on the GBM compositor backend. Phase 0 must probe the host.

### Official upstream evidence

Khronos `EGL_EXT_platform_device`
<https://registry.khronos.org/EGL/extensions/EXT/EGL_EXT_platform_device.txt>:

> This extension defines a method to create an EGLDisplay from an
> EGLDeviceEXT … This extension defines no method to create window
> or pixmap surfaces on the EGLDeviceEXT platform.

NVIDIA Jetson r36.5.2 Graphics APIs
<https://docs.nvidia.com/jetson/archives/r36.5.2/DeveloperGuide/SD/Graphics/GraphicsAPIs.html>
lists both `EGL_EXT_platform_device` and
`EGL_MESA_platform_surfaceless`.

NVIDIA desktop README 580.142 DRM KMS
<https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/kms.html>:

> Known Issues: … Buffer allocation and submission to DRM KMS using
> gbm is not currently supported.

Same README, GBM chapter
<https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/gbm.html>:

> Most Wayland compositors use the GBM API to initialize an
> EGLDisplay object directly on a GPU … The NVIDIA driver includes a
> GBM backend enabling the use of such software on NVIDIA GPUs.
> Requirements: DRM KMS must be enabled.

Khronos `EGL_MESA_platform_surfaceless`
<https://registry.khronos.org/EGL/extensions/MESA/EGL_MESA_platform_surfaceless.txt>:

> This platform's defining property is that it has no native
> surfaces … The platform is independent of any native window
> system. … `EGL_PLATFORM_SURFACELESS_MESA` … `<native_display>`
> parameter must be `EGL_DEFAULT_DISPLAY`.

### Implication for SplitDesk

| mechanism | Who uses it | Phase 0 |
| --- | --- | --- |
| `EGL_MESA_platform_surfaceless` + render node | Weston `-Bpipewire/--renderer=gl` | **primary**. Probe `EGL_EXTENSIONS` on `EGL_NO_DISPLAY`. |
| `EGL_EXT_platform_device` + pbuffer/EGLStream | Jetson/DRIVE samples | **not** used by upstream Weston. Do not claim Weston is an EGLDevice compositor. |
| GBM + `EGL_KHR_platform_gbm` + `nvidia-drm.modeset=1` | Weston `-Bdrm`, seat compositor | seat0 only. Conflicts with “do not take DRM master”. |
| Vulkan 1.3 ICD | Weston `--renderer=vulkan` | optional / experimental vs `gl`. |

If surfaceless GL init fails, fall back to `--renderer=pixman` and
set `EncoderKind::SoftwareFallback` with an explicit CPU-copy
warning. Do **not** invent an EGLDevice backend inside SplitDesk. Do
**not** flip `nvidia-drm.modeset=1` at session start (can break a
local Xorg seat). Jetson extension lists are not a GeForce
guarantee. Probe.

NVENC is not specified in the compositor/README material above.
Feeding NVENC is a media-path problem (Q5–Q6), not a Weston/NVIDIA
README guarantee.

---

## Q5. Can PipeWire frames stay DMA-BUF / GPU-resident into GStreamer?

**Verdict: PARTIAL**

PipeWire can carry video as `SPA_DATA_DmaBuf` (an fd that is *not*
assumed CPU-mappable). Official `pipewiresrc` wraps that fd with
GStreamer’s DMA-BUF allocator and defaults to *not* copying
(`DEFAULT_ALWAYS_COPY false`). That is **not** the same as staying
GPU-resident all the way to NVENC. `nvh264enc` does not advertise
`memory:DMABuf` on its sink. Official `cudaupload` also does not
list `memory:DMABuf` on its sink.

### Official upstream evidence

<https://docs.pipewire.org/page_design.html>:

> PipeWire was designed to: Be efficient for raw video using fd
> passing and audio with shared ringbuffers.

<https://docs.pipewire.org/group__spa__buffer.html>:

> `SPA_DATA_DmaBuf` — fd to dmabuf memory. This might not be readily
> mappable (unless the MAPPABLE flag is set) and should normally be
> handled with DMABUF apis.

<https://gstreamer.freedesktop.org/documentation/additional/design/dmabuf.html>:

> This kind of buffer/memory is usually stored in non-system memory
> … then its memory mapping for CPU access may impose a big overhead
> and low performance, or even impossible. … The *GstCapsFeatures*
> *memory:DMABuf* is usually used to negotiate DMA buffers.

`cudaupload` sink templates
<https://gstreamer.freedesktop.org/documentation/nvcodec/cudaupload.html>:
`video/x-raw`, `memory:GLMemory`, `memory:D3D11Memory`,
`memory:CUDAMemory` — **no** `memory:DMABuf`.

### Implication for SplitDesk

- Capture must negotiate `SPA_DATA_DmaBuf` + modifier with the
  Weston/PipeWire producer. Fallback is `SPA_DATA_MemFd` /
  `SPA_DATA_MemPtr` → `MemoryType::SystemMemory` and
  `cpu_copies_per_frame >= 1`.
- A DMA-BUF in GStreamer is still `MemoryType::DmaBuf`, not
  `CudaMemory`. `diagnostics media-path` must print the conversion
  hop (`DmaBuf` → `GLMemory` / `CudaMemory`). Do not claim
  zero-copy into NVENC.
- Keep `pipewiresrc` `always-copy=false`. Forcing a copy is the
  SoftwareFallback warning path.
- Latest-frame-wins: do **not** insert an unbounded `queue` after
  `pipewiresrc`. Depth-1 drop-old is the contract.

---

## Q6. Which GPU memory types does GStreamer NVENC (`nvh264enc`) accept?

**Verdict: PARTIAL**

`nvh264enc` is documented as “Encode H.264 video streams using
NVCODEC API CUDA Mode”. Of the types SplitDesk asked about,
`CUDAMemory` and `GLMemory` are supported; `DMABuf` is
**UNSUPPORTED**.

### Official upstream evidence

<https://gstreamer.freedesktop.org/documentation/nvcodec/nvh264enc.html>
sink pad template (verbatim structure):

```
video/x-raw(memory:CUDAMemory):
video/x-raw(memory:D3D12Memory):
video/x-raw(memory:GLMemory):
video/x-raw:
```

`memory:DMABuf` is not in that template.

NVIDIA Video Codec SDK 13.0
<https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html>:

> The NVIDIA Encoder supports use of the following types of devices:
> DirectX 9 / 10 / 11 / 12 … CUDA … OpenGL. … Use of CUDA device for
> Encoding is supported on Linux and Windows 10 and later … Use of
> the OpenGL device type for encoding is supported only on Linux.

No Linux DMA-BUF device type.

Low-latency knobs from the same `nvh264enc` page / SDK tuning table:
`bframes` default 0; `zerolatency`; `preset` `p1`–`p7` (legacy
`low-latency*` deprecated: “use p1~7 with tune”); `tune`
`ultra-low-latency`; `rc-lookahead` default 0; `multi-pass`;
`rc-mode` `cbr`. Cloud gaming / conferencing in a constrained
channel → Ultra-low latency + CBR.

### NVENC sink memory-type table

| Memory type | Official sink feature | Verdict |
| --- | --- | --- |
| `CUDAMemory` | `video/x-raw(memory:CUDAMemory)` | **SUPPORTED** — preferred Linux encoder input |
| `GLMemory` | `video/x-raw(memory:GLMemory)` | **SUPPORTED** — GPU-resident via GL; still not DMA-BUF |
| `D3D12Memory` | `video/x-raw(memory:D3D12Memory)` | **SUPPORTED** — Windows-oriented; not the Linux PipeWire path |
| `DMABuf` | *(not in pad template)* | **UNSUPPORTED** — must convert |
| system `video/x-raw` | no memory feature | **SUPPORTED** (CPU path) — upload; `cpu_copies_per_frame >= 1` |

### Implication for SplitDesk

- Linux encoder input is `CUDAMemory` (or `GLMemory` as a documented
  alternate). `MemoryPath.encoder_input` is **never** `DmaBuf` when
  the encoder is `nvh264enc`.
- Conversion hop is mandatory: PipeWire `DmaBuf` → GL or CUDA import
  → `nvh264enc`. Missing import → `SoftwareFallback` / system
  `video/x-raw`, log the CPU-copy warning, fill
  `cpu_copies_per_frame`.
- Encode settings: `bframes=0 zerolatency=true tune=ultra-low-latency
  preset=p1 rc-lookahead=0 multi-pass=disabled rc-mode=cbr
  gop-size=-1`. Do not re-enable B-frames “for quality”.
- `nvd3d11h264enc` / `D3D12Memory` are Windows host-path concerns.

---

## Q7. How should Weston / libweston do per-session remote input (not uinput / xdotool)?

**Verdict: PARTIAL**

Official Weston documents that **the back-end owns input**. The
PipeWire back-end is explicitly **output-only**. Headless is **no
input**. The only stock remote-input back-end is **RDP**, which is
not SplitDesk’s protocol.

The supported per-session path is: one Weston (one `weston_seat`)
inside the user’s session, and inject pointer/key/scroll into
**that seat** with libweston’s public `weston_*_send_*` APIs (or a
tiny custom back-end/plugin). That stays inside the compositor
process. It does not create a global kernel device (`uinput`) and
does not drive X11 (`xdotool`).

libei is the official Wayland-stack emulated-input protocol. Weston
16.0.90’s published table of contents does **not** list a stock
libei / EIS back-end. `weston -Bpipewire` does not already accept
libei.

### Official upstream evidence

<https://wayland.pages.freedesktop.org/weston/toc/running-weston.html>:

> Ultimately, the back-end is responsible for handling the input and
> generate an output. … **pipewire** — run without input, output
> into a PipeWire node … **headless** — run without input or output
> … **rdp** — run as an RDP server without local input or output.

Public per-seat send APIs in
<https://gitlab.freedesktop.org/wayland/weston/-/raw/master/include/libweston/libweston.h>:
`weston_pointer_send_motion` / `send_button` / `send_axis` /
`send_frame`, `weston_keyboard_send_key` / `send_modifiers`,
`weston_seat_get_pointer` / `get_keyboard`.

libei
<https://libinput.pages.freedesktop.org/libei/>:

> the server side, typically a Wayland compositor, is called the
> “EIS Implementation”. … Note how the EI client is roughly
> equivalent to a physical input device coming from the kernel …
> To Wayland clients, they are indistinguishable from real devices.

### Implication for SplitDesk

- Spawn Weston as the session UID (`WAYLAND_DISPLAY=splitdesk-<id>`).
  Do **not** take seat0 DRM master.
- Phase 0 input against stock Weston: a libweston plugin / tiny
  custom back-end that maps SplitDesk `PointerMotion` /
  `PointerButton` / `Key` / `Scroll` onto `weston_pointer_send_*`
  and `weston_keyboard_send_key` for **that session’s**
  `weston_seat`. Motion is latest-wins; buttons and keys are
  reliable. On `Disconnect`, release every virtual key/button via
  `send_key` / `send_button` (compositor-local).
- Isolation: different users never share `wayland_display` or the
  same `weston_seat`.
- Forbidden: global `uinput`, `xdotool`, injecting into seat0 of
  the login session, RDP/VNC as the SplitDesk transport.
- libei remains the **standard** compositor-side pointer if SplitDesk
  later grows an EIS plugin (Q8). Do not pretend
  `weston -Bpipewire` already speaks it.

---

## Q8. Where should libei / libeis sit (compositor vs portal vs streamer)?

**Verdict: PARTIAL**

Official layering is unambiguous. SplitDesk cannot treat it as fully
supported on the planned Weston host because Weston 16.0.90 does not
implement EIS (tree-wide grep in the Q8–Q10 note: no `libei` /
`libeis` / `ConnectToEIS`).

### Official upstream evidence

libei README / <https://libinput.pages.freedesktop.org/libei/libraries/index.html>:

> In the Wayland stack, the EIS server component is part of the
> compositor, the EI client component is part of the Wayland client.

libeis header:

> libeis is the server-side module. This API should be used by
> processes that have control over input devices, e.g. Wayland
> compositors.

xdg-desktop-portal RemoteDesktop v2
<https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html>:

> **EIS (recommended):** Call `ConnectToEIS()` after starting the
> session to obtain a file descriptor for a libei sender context.
> Input events are sent via the EI protocol. Once an EIS connection
> is established, the `Notify*` D-Bus methods must not be used.

libei README portal handshake:

> from then on, `libei` and `libeis` talk directly to each other,
> the portal has no further influence.

Xwayland already consumes libei as a **client** for XTEST
(`xwayland-xtest.c`). That does not move libeis into Xwayland.

### Implication for SplitDesk

| Piece | Process | Role |
| --- | --- | --- |
| **libeis** | Wayland compositor | Server. Owns seats/devices. Filter, pause, inject into the compositor input stack. |
| **libei** | Streamer (and Xwayland XTEST) | Client. Sender context. `ei_new_sender` + `ei_setup_backend_fd`. |
| **xdg-desktop-portal RemoteDesktop** | Session broker | Auth + one-shot `ConnectToEIS()` FD handover. **Not** on the per-event path after connect. **Not** the default for a dedicated per-user Weston. |

- **Do not put libeis in the streamer.** The compositor must remain
  in control.
- **Do not put libei in the compositor.**
- A SplitDesk-owned Weston should mint an EIS FD internally (or a
  private UNIX socket) and pass it to the streamer. Portal `Start()`
  is designed for a user-consent desktop session (Mutter/KWin/etc.).
- Until Weston grows libeis (or SplitDesk uses a compositor that
  already has it), Linux input against Weston is **not** the
  portal/libei path. Options: add libeis to Weston, switch
  compositor, or use compositor-private virtual-seat APIs (Q7).
- Do not use global uinput or xdotool as the primary input path.
  libei README: uinput is an EIS-server implementation detail
  requiring `/dev/uinput` (usually root); XTEST has no useful
  separation.

Dedicated SplitDesk Weston session (no portal):

```
remote client
    |  SplitDesk Input (motion latest-wins; button/key reliable)
    v
streamer  --libei sender / ei_setup_backend_fd-->  compositor libeis
                                                     |
                                                     v
                                              wl_seat / Xwayland
```

---

## Q9. Are XWayland apps GPU-accelerated under per-user Weston / headless?

**Verdict: PARTIAL**

Hardware GLX/EGL via DRI3+glamor is the designed Xwayland path. It
is not guaranteed under Weston headless, and NVIDIA adds extra
requirements and missing features.

### Official upstream evidence

`weston.ini(5)`: `xwayland=true` loads the XWayland module. Do
**not** put `xwayland.so` in `modules=` (Weston treats that as
fatal). Weston spawns Xwayland **rootless** with listen FDs and
`-wm`. `WAYLAND_DISPLAY=splitdesk-<id>` is enough for that Xwayland
to attach.

Xwayland man
<https://gitlab.freedesktop.org/xorg/xserver> (`Xwayland.man`):

> `XWAYLAND_NO_GLAMOR` — disable glamor and DRI3 support in
> Xwayland, for testing purposes.
> `-shm` — Force the shared memory backend instead of glamor (if
> available) for passing buffers to the Wayland server.

xwayland-screen.c fallback strings: `xwayland glamor: failed to
setup GBM backend, falling back to sw accel` / `Failed to initialize
glamor, falling back to sw`.

NVIDIA README 570.133.07 Chapter 40 (GBM)
<https://download.nvidia.com/XFree86/Linux-x86_64/570.133.07/README/gbm.html>
(Q8–Q10 note; desktop 580.142 GBM chapter is the same architecture):

> Xwayland uses the GLAMOR backend to accelerate X application
> rendering using EGL. … GLAMOR acceleration is required for good
> performance when running accelerated Vulkan or OpenGL X11
> applications. … To use the GBM path with the NVIDIA driver's GBM
> backend … Xwayland versions 21.1.3 and above satisfy this
> requirement.

Appendix L / wayland-issues: front-buffer GLX does not work with
Xwayland; indirect GLX is not supported; hardware overlays cannot be
used by GLX apps with Xwayland; NvFBC does not work with Xwayland.

NVIDIA GBM **compositor** support is specified for DRM KMS +
`libgbm.so.1`. Weston **headless / pipewire** is surfaceless EGL,
not GBM/KMS. That is a documented compositor path mismatch.

### Implication for SplitDesk

- Enable Weston `xwayland=true`, spawn as the session UID,
  `WAYLAND_DISPLAY=splitdesk-<id>`.
- Prefer `--renderer=gl` so the compositor can import GPU buffers;
  pixman forces extra copies if clients present dmabuf that must be
  read back.
- Do not set `XWAYLAND_NO_GLAMOR` or `-shm` in production.
- Client GL can be GPU-backed even when Weston composites in
  surfaceless EGL **if** Xwayland GBM/DRI3 comes up and Weston can
  import dmabuf. If either side lacks dmabuf/GBM, frames go SHM
  (CPU copy) — measure it.
- Diagnose with Xwayland logs and client `glxinfo`/`eglinfo`;
  llvmpipe means the DRI3/GBM path failed.
- Treat NVIDIA + Weston-headless composition as unproven. Prefer
  Mesa/Intel/AMD surfaceless, or a non-master render-node setup that
  still does not steal seat0. NVIDIA-accelerated **Xwayland
  clients** may still work (Xwayland talks GBM/EGL itself).
- Never claim zero-copy unless `diagnostics media-path` shows GPU
  memory types end-to-end.

---

## Q10. Can CloudCompare (Qt + OpenGL, often X11) run in this architecture?

**Verdict: UNKNOWN**

No official CloudCompare statement about Weston, headless, XWayland,
or remote-desktop compositors was found. No CloudCompare process was
run. Architectural prerequisites exist; that is not a run result.

### Official upstream evidence

CloudCompare BUILD.md (2.14+)
<https://github.com/CloudCompare/CloudCompare/blob/master/BUILD.md>:

> The main dependency of CloudCompare is Qt. CloudCompare 2.14+
> requires Qt 6.

The 3D view is a `QOpenGLWidget`
(`libs/qCC_glWindow/include/ccGLWindow.h`). No `WAYLAND_DISPLAY`,
`QT_QPA_PLATFORM`, XWayland, or Weston references in that BUILD.md /
`ccGLWindow.h` / `qCC/main.cpp` path.

Qt 6 QPA
<https://doc.qt.io/qt-6/qpa.html> /
<https://doc.qt.io/qt-6/qguiapplication.html>:

> `-platform platformName[:options]` … Overrides the
> `QT_QPA_PLATFORM` environment variable.
> `wayland` is a platform plugin for the Wayland display server
> protocol…
> `xcb` is a plugin for the X11 window system…

### Implication for SplitDesk

How it *could* run (not tested):

1. Native Wayland: `QT_QPA_PLATFORM=wayland` inside
   `WAYLAND_DISPLAY=splitdesk-<id>`. Needs Qt Wayland EGL and the
   protocols Qt wants (xdg-shell, linux-dmabuf, …). Missing
   protocols → startup fail, not silent software GL.
2. XWayland (historical X11 default): `QT_QPA_PLATFORM=xcb` +
   Weston `xwayland=true`. GPU vs llvmpipe is Q9, not a
   CloudCompare guarantee.
3. Headless Weston without a `wl_seat`: enable Weston’s `fake_seat`;
   remote input still needs Q7/Q8.

- Ship **both** launch paths: prefer `wayland` when Qt Wayland EGL
  is installed; fall back to `xcb` + Xwayland.
- Do **not** advertise CloudCompare as a supported GPU app until a
  real session shows a hardware renderer (not llvmpipe) and
  interactive 3D input.
- Do not force EGLFS (`qeglfs`); that bypasses the compositor.
- Files stay on the host; the process is a normal user app on that
  Weston’s `WAYLAND_DISPLAY`.

---

## Q11. DXGI Desktop Duplication / Windows Graphics Capture → D3D surface → hardware encoder

**Verdict: SUPPORTED** for keeping a D3D11 texture from capture into
a GPU encoder via Media Foundation DXGI buffers.
**PARTIAL** if the claim is “Microsoft documents NVENC D3D11 as the
encoder” or “inbox H.264 MFT consumes D3D surfaces.”

Do **not** use `ID3D11DeviceContext::Map` / GDI `GetDC` /
`CopyToMemory` as the primary path. Inbox `mfh264enc.dll` is CPU.

### Official upstream evidence

Desktop Duplication
<https://learn.microsoft.com/windows/win32/direct3ddxgi/desktop-dup-api>:

> Because apps receive updates to the desktop image in a DXGI
> surface, the apps can use the full power of the GPU to process
> the image updates.

`AcquireNextFrame` returns `IDXGIResource`; Microsoft’s sample
`QueryInterface`s it to `ID3D11Texture2D`.

Windows Graphics Capture
<https://learn.microsoft.com/uwp/api/windows.graphics.capture.direct3d11captureframe.surface>:

> The Direct3D surface on which the frame was drawn. Property
> Value: `IDirect3DSurface`

`CreateForMonitor` is Windows 10 1903+ (build 18362).

Wrap as an MF sample
<https://learn.microsoft.com/windows/win32/api/mfapi/nf-mfapi-mfcreatedxgisurfacebuffer>:

> Creates a media buffer to manage a Microsoft DirectX Graphics
> Infrastructure (DXGI) surface. `riid` … must be
> **IID_ID3D11Texture2D** or **IID_ID3D12Resource**.

Inbox H.264 encoder
<https://learn.microsoft.com/windows/win32/medfound/h-264-video-encoder>:

> The input media type must have one of the following subtypes:
> **MFVideoFormat_I420**, **MFVideoFormat_IYUV**,
> **MFVideoFormat_NV12**, **MFVideoFormat_YUY2**,
> **MFVideoFormat_YV12**.
>
> If a certified hardware encoder is present, it will generally be
> used instead of the inbox system encoder for Media Foundation
> related scenarios.

Hardware MFTs
<https://learn.microsoft.com/windows/win32/medfound/hardware-mfts>:

> If two MFTs represent the same physical device, they can exchange
> data within the hardware … There is no need to copy the data into
> system memory and then back to the device.

Microsoft Learn does **not** document NVIDIA Video Codec SDK
(`nvEncodeAPI`, `ID3D11Texture2D` registration).

`DuplicateOutput` documented failures include `E_ACCESSDENIED`
(secure desktop / LOCAL_SYSTEM), `DXGI_ERROR_SESSION_DISCONNECTED`,
and a default cap of four concurrent duplication apps.
`DXGI_ERROR_ACCESS_LOST` on desktop switch / mode change.

The texture from `AcquireNextFrame` / WGC is **not** documented as
created with `D3D11_RESOURCE_MISC_SHARED_NTHANDLE`. Primary GPU
path is: use it on the **same D3D11 device** that was passed to
`DuplicateOutput` / `Direct3D11CaptureFramePool.Create`, then
`CopyResource` (GPU) into an encoder or shareable texture.

### Implication for SplitDesk

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
        (inbox mfh264enc.dll is this path)
```

Capture **must** run inside the target interactive WTS session (Q12),
not in the daemon’s Session 0. WGC yellow border is system UI; do
not try to hide it.

`EncoderKind::Nvenc` may use the vendor MFT or the vendor SDK, but
**must not claim a Microsoft-documented NVENC API**.

| Field | Typical hardware path | SoftwareFallback |
| --- | --- | --- |
| `render_output` | `D3D11` (DWM) | `D3D11` |
| `capture` | `D3D11` | `D3D11` |
| `conversion` | `D3D11` | `SystemMemory` |
| `encoder_input` | `D3D11` | `SystemMemory` |
| `cpu_copies_per_frame` | `0` | `≥ 1` |

---

## Q12. Bind a per-session agent to the correct WTS session ID

**Verdict: SUPPORTED** — enumerate sessions, query identity, take
the user token, `CreateProcessAsUser` so the child runs in that
session’s `WinSta0\Default`.
**UNSUPPORTED** — creating a **second concurrent interactive
session** on Windows 10/11 client SKUs. Return
`MULTIUSER_NOT_SUPPORTED_BY_HOST_OS`. Do not patch `termsrv.dll` or
use RDP Wrapper.

### Official upstream evidence

<https://learn.microsoft.com/windows/win32/termserv/terminal-services-sessions>:

> Each remote desktop session is associated with an interactive
> window station. The only supported window station name for an
> interactive window station is "WinSta0"; therefore each session
> is associated with its own "WinSta0" window station.

`WTSQueryUserToken`
<https://learn.microsoft.com/windows/win32/api/wtsapi32/nf-wtsapi32-wtsqueryusertoken>:

> To call this function successfully, the calling application must
> be running within the context of the **LocalSystem account** and
> have the **SE_TCB_NAME** privilege. … Any program running in the
> context of a service will have a session identifier of zero (0).

`CreateProcessAsUserW`
<https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw>:

> Terminal Services: The process is run in the session specified in
> the token. … To enable user interaction with the new process, you
> must specify … **"winsta0\\default"** in the **lpDesktop** member
> of the **STARTUPINFO** structure.

Session 0 isolation
<https://learn.microsoft.com/windows/win32/services/interactive-services>:

> Services cannot directly interact with a user as of Windows
> Vista. All services run in Terminal Services session 0. …
> Create a separate hidden GUI application and use the
> CreateProcessAsUser function to run the application within the
> context of the interactive user.

`OSVERSIONINFOEXW.wSuiteMask`
<https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-osversioninfoexw>:

> **VER_SUITE_SINGLEUSERTS** (0x00000100) — Remote Desktop is
> supported, but only one interactive session is supported. This
> value is set unless the system is running in application server
> mode.

Windows Enterprise multi-session is AVD-only
<https://learn.microsoft.com/azure/virtual-desktop/windows-multisession-faq>:

> We don't allow customers to run Windows Enterprise multi-session
> in production environments outside of the Azure Virtual Desktop
> service. … It's against the licensing agreement to run Windows
> multi-session outside of the Azure Virtual Desktop service for
> production purposes.

RDS CALs (Windows Server session hosts)
<https://learn.microsoft.com/windows-server/remote/remote-desktop-services/rds-client-access-license>:

> Each user and device that connects to a Remote Desktop Services
> session host … needs a Remote Desktop Services (RDS) client
> access license (CAL).

There is **no** Microsoft-documented supported bypass of the
one-interactive-session client-OS boundary.

### Implication for SplitDesk

Spawn sequence (existing session only):

1. `WTSEnumerateSessionsExW` → find `WTSActive` session whose
   `WTSUserName` matches.
2. If host is Win10/11 client (`VER_NT_WORKSTATION` +
   `VER_SUITE_SINGLEUSERTS`) and a SplitDesk session already
   occupies that interactive session, or the caller asked for a
   **second** interactive desktop: return
   `MULTIUSER_NOT_SUPPORTED_BY_HOST_OS`.
3. `WTSQueryUserToken(sessionId)` from LocalSystem.
4. `LoadUserProfile` + `CreateEnvironmentBlock`.
5. `STARTUPINFO.lpDesktop = L"winsta0\\default"`;
   `bInheritHandles = FALSE`.
6. `CreateProcessAsUserW` → capture/encode/input agent. Store
   `windows_session_id`.
7. Isolation: two SplitDesk users must not share
   `windows_session_id`.

Detect with `RtlGetVersion` + `wProductType` / `wSuiteMask` +
`GetProductInfo`. Do not trust manifested `GetVersionEx`.

| Host | `session_support` | Extra interactive session |
| --- | --- | --- |
| Windows 10/11 Home/Pro/Enterprise (workstation, `VER_SUITE_SINGLEUSERTS`) | `WindowsSingleInteractive` | **No.** Second create → `MULTIUSER_NOT_SUPPORTED_BY_HOST_OS`. |
| Windows Enterprise multi-session | AVD-only | Not an on-prem SplitDesk host. |
| Windows Server **with** RD Session Host (application server mode) | `WindowsServerRds` | OS role, **licensed**. Each user/device needs an RDS CAL. SplitDesk does not issue CALs. |

Must not: patch `termsrv.dll`; RDP Wrapper; treat Fast User
Switching disconnected sessions as extra concurrent interactive
desktops; run capture/input in Session 0; use
`SERVICE_INTERACTIVE_PROCESS`; ship Windows Enterprise multi-session
outside AVD; imply RDS CALs unlock multi-session on Win10/11 client
editions.

---

## Recommended Phase 0 architecture

### Linux session compositor

One Weston **per session**, as the session UID, never root, never
`-Bdrm`, never `--seat=seat0`:

```
weston -Bpipewire --renderer=gl \
       --socket=splitdesk-<id> \
       --width=1920 --height=1080 \
       --idle-time=0 \
       --config=/path/to/session-weston.ini
```

Environment (session UID):

```
XDG_RUNTIME_DIR=/run/user/<uid>   # 0700, owned by that UID
# unset WAYLAND_DISPLAY so Weston does not nest
XDG_SESSION_TYPE=wayland
```

`weston.ini` (FPS is not a Weston CLI flag):

```
[core]
require-input=false
xwayland=true

[output]
name=pipewire
mode=1920x1080@60
```

Fallback if PipeWire is down: `-Bheadless --renderer=gl` (capture
then needs another API; not a PipeWire node). Always pass
`--renderer=gl`. `WESTON_RENDERER_AUTO` on pipewire is Pixman; on
headless it is noop.

Require Weston ≥13. Session process: `render` group on
`/dev/dri/renderD*` only. Probe `EGL_MESA_platform_surfaceless`
before claiming GPU composition on NVIDIA. Do not flip
`nvidia-drm.modeset=1` at session start.

Forbidden: `-Bdrm`, `-Bdrm,pipewire` for extra sessions, running
Weston as root, world-writable `XDG_RUNTIME_DIR`, inheriting the
seat `WAYLAND_DISPLAY`.

### Linux media path

```
Weston GL FBO / linear DMA-BUF or MemFd
        │
        v
PipeWire  (CaptureKind::PipeWire, always-copy=false, depth 1)
        │  SPA_DATA_DmaBuf  →  MemoryType::DmaBuf
        │  SPA_DATA_MemFd   →  MemoryType::SystemMemory, copies ≥ 1
        v
conversion hop (mandatory for NVENC)
        │  DmaBuf  →  GLMemory or CUDAMemory
        │  (not “zero-copy into the encoder”)
        v
nvh264enc  encoder_input = CUDAMemory (preferred) or GLMemory
           bframes=0 zerolatency=true tune=ultra-low-latency
           preset=p1 rc-lookahead=0 multi-pass=disabled
           rc-mode=cbr gop-size=-1
```

**DMA-BUF is not an `nvh264enc` sink type.** If the import hop is
missing, `EncoderKind::SoftwareFallback`: log CPU-copy warning,
measure copy latency, fill `cpu_copies_per_frame`.

`diagnostics media-path` prints the real `MemoryType` at
`render_output`, `capture`, `conversion`, `encoder_input`.

### Linux input

Target layering: **libeis in the compositor, libei in the streamer**.
Portal only when attaching to an existing desktop (Mutter/KWin) that
already implements EIS.

Weston 16.0.90 does not ship EIS. Phase 0 against stock Weston is
therefore compositor-local `weston_*_send_*` on that session’s
`weston_seat` (Q7), with libeis as the follow-on plugin — not as
something `-Bpipewire` already provides.

Forbidden: global `uinput`, `xdotool`, seat0 of the login session,
RDP/VNC as transport.

### X11 apps / CloudCompare

Weston `xwayland=true`, rootless Xwayland, no `XWAYLAND_NO_GLAMOR`.
Prefer Qt `wayland`; fall back to `xcb` + Xwayland. CloudCompare is
**UNKNOWN** until a real session shows a hardware renderer and
interactive 3D input. Do not advertise it as a supported GPU app.

### Windows host

Win10/11: **one** interactive session (`WindowsSingleInteractive`).
A second create returns `MultiUserNotSupportedByHostOs`. No
`termsrv.dll` patch, no RDP Wrapper, no RDS licensing bypass, no
Session-0 capture/input.

Daemon (Session 0, LocalSystem) binds the agent with
`WTSEnumerateSessionsEx` + `WTSQueryUserToken` +
`CreateProcessAsUser` into `WinSta0\Default` of the existing
interactive session. Capture (`Dxgi` or `WindowsGraphicsCapture`)
and encode run **in that session**.

Windows media path: D3D11 texture → GPU blit/VideoProcessor →
`MFCreateDXGISurfaceBuffer` → certified hardware MFT. Inbox
`mfh264enc.dll` is CPU YUV — SoftwareFallback only. Do not claim a
Microsoft-documented NVENC D3D11 API.

Windows Server RDSH is a licensed OS role (`WindowsServerRds`);
each user/device needs an RDS CAL. SplitDesk does not issue CALs.
Windows Enterprise multi-session is AVD-only, not an on-prem host.

### What Phase 0 will not claim

- Zero-copy from Weston into NVENC.
- DMA-BUF as `nvh264enc` input.
- NVIDIA desktop GBM KMS submit as supported (README 580.142 Known
  Issues).
- Weston `-Bpipewire` already speaking libei.
- CloudCompare GPU-validated.
- Inbox `mfh264enc` as a D3D/hardware path.
- A second interactive desktop on Windows 10/11 client SKUs.
