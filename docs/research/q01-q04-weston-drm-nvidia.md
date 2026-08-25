# Q01–Q04 — Weston, DRM master, NVIDIA headless

Status: source-verified against upstream Weston 16.0.90
(`https://gitlab.freedesktop.org/wayland/weston`, HEAD `6161d79`,
`meson.build` `version: '16.0.90'`), official Weston docs 16.0.90,
NVIDIA Linux driver README 580.142, Khronos EGL specs, Linux DRM
uAPI, PipeWire 1.6.8 stream docs, NVIDIA Jetson Linux r36.5.2.

No marketing claims. UNKNOWN is used where desktop NVIDIA README
contradicts the Jetson EGL-extension list.

Recommended Phase 0 compositor command (does **not** take seat0 DRM
master):

```
weston -Bpipewire --renderer=gl --socket=splitdesk-<id> \
       --width=1920 --height=1080 --idle-time=0
```

Environment (as the session UID, never root):

```
XDG_RUNTIME_DIR=/run/user/<uid>   # must exist, mode 0700, owned by that UID
# unset WAYLAND_DISPLAY so Weston does not nest
XDG_SESSION_TYPE=wayland
```

FPS is **not** a Weston CLI flag. Put it in a per-session `weston.ini`:

```
[output]
name=pipewire
mode=1920x1080@60
```

Do **not** pass `-Bdrm` or `--seat=seat0`.

---

## Q1. Can the Weston PipeWire backend run together with a GPU renderer?

### Verdict: SUPPORTED

Backend and renderer are separate libweston objects. The PipeWire
backend is an output path (a PipeWire node per output). Composition
is done by a renderer (`gl`, `vulkan`, or `pixman`). They are used
together in one Weston process.

### Evidence

Official Weston 16.0.90 docs, *Running Weston*
<https://wayland.pages.freedesktop.org/weston/toc/running-weston.html>
(source `doc/sphinx/toc/running-weston.rst`):

> Available back-ends:
>
> * **pipewire** -- run without input, output into a PipeWire node
>
> The job of gathering all the surfaces … is performed by a *renderer*.
> … There are OpenGL ES and Vulkan renderers, which will often be
> accelerated by your GPU when suitable drivers are installed.
> … You can select between these with the `--renderer=gl`,
> `--renderer=vulkan` and `--renderer=pixman` arguments when starting
> Weston.

`weston(1)` (Debian unstable package weston 16.0.0-1, same text as
upstream `man/weston.man`):

> **pipewire** — The PipeWire backend runs in memory without the need
> of graphical hardware and creates a PipeWire node for each output.
> It can be used to capture Weston outputs for processing with another
> application.
>
> `--renderer=renderer` — Select which renderer to use for Weston's
> internal composition.

Upstream `libweston/backend-pipewire/pipewire.c` (master) initializes
the GL renderer on the PipeWire backend with the Mesa surfaceless
EGL platform — not with the DRM/GBM platform:

```c
case WESTON_RENDERER_GL: {
    const struct gl_renderer_display_options options = {
        .egl_platform = EGL_PLATFORM_SURFACELESS_MESA,
        .formats = backend->formats,
        .formats_count = backend->formats_count,
    };
    ret = weston_compositor_init_renderer(compositor,
                                          WESTON_RENDERER_GL,
                                          &options.base);
```

The same file accepts GL, Vulkan, and Pixman when PipeWire is loaded
as a secondary backend (renderer already created by the primary):

```c
if (compositor->renderer) {
    switch (compositor->renderer->type) {
    case WESTON_RENDERER_PIXMAN:
    case WESTON_RENDERER_GL:
    case WESTON_RENDERER_VULKAN:
        break;
```

Output enablement for GL uses an FBO, not a KMS plane
(`pipewire_output_enable_gl` → `renderer->gl->output_fbo_create`).
When the renderer exposes `dmabuf_alloc`, the backend offers a
`SPA_DATA_DmaBuf` format with **only** `DRM_FORMAT_MOD_LINEAR` and
this comment: `/* TODO: Add support for modifier discovery and
negotiation. */`.

Multi-backend (Weston ≥13, still documented in 16.0.90):

> Some back-ends can be selected via a comma-separated list …
> example `-B drm,vnc`. The first back-end … is the *primary*
> back-end. It creates the renderer … The PipeWire and VNC backends
> support being loaded as secondary backends.

That is `-B drm,pipewire` (local KMS + PipeWire stream), **not**
required for SplitDesk. It *does* take DRM master on the primary.

PipeWire stream API used by the backend
<https://docs.pipewire.org/page_streams.html> (PipeWire 1.6.8):

> Produce a stream to PipeWire. This is a `PW_DIRECTION_OUTPUT` stream.
> … `pw_stream_connect()`. … `pw_stream_dequeue_buffer()` gives an
> empty buffer that can be filled. Filled buffers should be queued
> with `pw_stream_queue_buffer()`.

Weston calls `pw_stream_connect(..., PW_DIRECTION_OUTPUT, ...,
PW_STREAM_FLAG_DRIVER | PW_STREAM_FLAG_ALLOC_BUFFERS, ...)`.

### Implication for SplitDesk Phase 0

- Use **one** Weston process per session: `-Bpipewire --renderer=gl`.
- PipeWire is the capture source (`CaptureKind::PipeWire`). It is not
  a second compositor.
- Do **not** also load `-Bdrm`. That is only for a local monitor.
- `--renderer` is mandatory. `WESTON_RENDERER_AUTO` on the PipeWire
  backend falls through to **Pixman** (CPU). That is a software
  compositor, not GPU composition:

```c
case WESTON_RENDERER_AUTO:
case WESTON_RENDERER_PIXMAN:
    ret = weston_compositor_init_renderer(compositor,
                                          WESTON_RENDERER_PIXMAN,
                                          NULL);
```

- Zero-copy into NVENC is **not** documented as complete. The backend
  only advertises linear DMA-BUF; modifier negotiation is an upstream
  TODO. Treat DMA-BUF as best-effort; expect a CPU copy path
  (`SPA_DATA_MemFd`) if the renderer has no `dmabuf_alloc` or the
  consumer rejects linear.

### Risks

- Distro Weston older than 13: no multi-backend; GL on PipeWire may
  be missing. Phase 0 should require Weston ≥13 (GL on pipewire) and
  prefer ≥14/15/16 matching current docs.
- NVIDIA desktop driver README never names `EGL_MESA_platform_surfaceless`.
  Jetson r36.5.2 *does* list it (see Q4). Probe the EGL client
  extension string before claiming GPU composition on a given host.
- Default PipeWire head is **640×480 @ 30 Hz**
  (`default_config` in `pipewire.c`). `--width`/`--height` override
  size; FPS comes from `weston.ini` `mode=WxH@R` via
  `parse_simple_mode()`.
- `PW_STREAM_FLAG_DRIVER` makes Weston a graph driver. A per-user
  PipeWire daemon must be running in that user's session.

---

## Q2. Can each Linux user start an independent Weston instance (socket, `XDG_RUNTIME_DIR`)?

### Verdict: SUPPORTED

Weston is a per-process compositor. Isolation is the Wayland socket
path: `$XDG_RUNTIME_DIR/<socket-name>`. systemd already gives each
logged-in user a private `XDG_RUNTIME_DIR`. Multiple Westons for the
*same* UID need distinct `--socket` names.

### Evidence

Official *Running Weston*:

> Weston creates its unix socket file (for example, wayland-1) in the
> directory specified by the required environment variable
> `$XDG_RUNTIME_DIR`. Clients use the same variable to find that
> socket. Normally this should already be provided by systemd.

`weston(1)` ENVIRONMENT:

> **XDG_RUNTIME_DIR** — The directory for Weston's socket and lock
> files. Wayland clients will automatically use this.
>
> **WAYLAND_DISPLAY** — The name of the display (socket) of an already
> running Wayland server, without the path. The directory path is
> always taken from `XDG_RUNTIME_DIR`.
>
> `-S name, --socket=name` — Weston will listen in the Wayland socket
> called *name*. Weston will export `WAYLAND_DISPLAY` with this value
> in the environment for all child processes.

Upstream `frontend/main.c` `verify_xdg_runtime_dir()`:

- missing `XDG_RUNTIME_DIR` → fatal exit
- not a directory → fatal exit
- mode must be `0700` and owner must be `getuid()` (warning if not)

`weston_create_listening_socket()`:

- if `--socket=NAME` is set: `wl_display_add_socket(display, NAME)`
  then `setenv("WAYLAND_DISPLAY", NAME, 1)`
- else it tries `wayland-1` … `wayland-32`

Nested example from the same man page:

```
WAYLAND_DISPLAY=wayland-0 weston -Swayland-1
```

If `WAYLAND_DISPLAY` is already set and `-B` is omitted, the
**default** backend becomes `wayland` (nested). SplitDesk must either
pass `-Bpipewire`/`-Bheadless` or unset `WAYLAND_DISPLAY` before exec.

Official systemd user-service example in *Running Weston* listens on
`%t/wayland-0` (`%t` = user runtime dir) and starts Weston as that
user (`Type=notify`, `systemd-notify.so`). A second user gets a
different `%t`.

### Implication for SplitDesk Phase 0

Per session, as the **session UID** (never root):

| item | value |
| --- | --- |
| `XDG_RUNTIME_DIR` | `/run/user/<uid>` if a user session exists; else a dedicated dir `0700` owned by that UID (not `/tmp` shared) |
| `--socket` / `WAYLAND_DISPLAY` | `splitdesk-<SessionId>` (e.g. `splitdesk-sd-001`) |
| socket path | `$XDG_RUNTIME_DIR/splitdesk-<id>` |
| compositor UID | the session user; `systemd-run --uid=…` only when the daemon is root |

Two users may both use socket name `splitdesk-sd-001` because the
directories differ. Two sessions for the *same* user must not share a
socket name. That matches the isolation rule: different users cannot
share `wayland_display`.

`XDG_RUNTIME_DIR` is also where PipeWire's user daemon socket lives.
A Weston PipeWire backend started as user U talks to U's PipeWire,
not to another user's.

### Risks

- No login session → no `/run/user/<uid>`. Creating a fake runtime
  dir is allowed by Weston if it is a directory mode `0700` owned by
  the UID; it will not create a PipeWire daemon. Phase 0 must start
  (or reuse) a user PipeWire when using `-Bpipewire`.
- If the daemon process keeps the seat compositor's `WAYLAND_DISPLAY`
  in the environment, Weston nests instead of becoming a session
  compositor. Always pass `-B…` and strip inherited `WAYLAND_DISPLAY`.
- `DISPLAY` set + `WAYLAND_DISPLAY` unset + no `-B` → default backend
  is `x11`. Always pass `-B`.
- Socket names are not authenticated. Isolation is directory
  permissions (`0700`). Do not use a world-writable runtime dir.
- Weston does not implement “switch user” or a shared multi-seat
  socket. One process, one socket, one `XDG_RUNTIME_DIR`.

---

## Q3. How to avoid every session competing for physical DRM master? (EGLDevice / GBM / headless / nvidia-drm)

### Verdict: SUPPORTED

Do not use the DRM/KMS backend. Use PipeWire or headless. GPU access
then goes through a **render node** (`/dev/dri/renderD*`), which the
kernel documents as having **no DRM-Master**. Physical `card*` master
stays with the seat0 compositor (or unused).

### Evidence

`weston(1)` on the backends that do **not** need KMS:

> **pipewire** — runs in memory without the need of graphical hardware
>
> **headless** — The headless backend runs in memory.

`weston-drm(7)` on the backend that **does** take the device:

> The DRM backend is the native Weston backend for systems that
> support the Linux kernel DRM, kernel mode setting (KMS), and evdev
> input devices. … It also relies on the Mesa GBM interface. … With
> the DRM backend, weston runs without any underlying windowing
> system. The backend uses the Linux KMS API to detect connected
> monitors. … The backend chooses the DRM graphics device first based
> on seat id.

Official *Running Weston* (stand-alone / extra GPU):

> By default Weston will use the default seat named `seat0` …
> `--seat` …
> A graphics card is required to be a part of the seat …
> `SEATD_VTBOUND=0 ./weston -Bdrm --seat=seat-insecure`

That extra-seat path still takes **a** DRM master (on the unused
card). It is not how to share one GPU among many sessions.

Linux DRM uAPI, *Render nodes*
<https://docs.kernel.org/gpu/drm-uapi.html>
(source `Documentation/gpu/drm-uapi.rst` in torvalds/linux):

> With the increased use of offscreen renderers and GPGPU
> applications, clients no longer require running compositors or
> graphics servers to make use of a GPU. But the DRM API required
> unprivileged clients to authenticate to a DRM-Master prior to
> getting GPU access. To avoid this step … render nodes were
> introduced. Render nodes solely serve render clients, that is, no
> modesetting or privileged ioctls can be issued on render nodes.
>
> … a separate render node called `renderD<num>`. …
>
> Besides dropping all modeset/global ioctls, render nodes also drop
> the DRM-Master concept. There is no reason to associate render
> clients with a DRM-Master as they are independent of any graphics
> server. Besides, they must work without any running master, anyway.

Same document, *Primary Nodes, DRM Master and Authentication*:

> In addition only one `drm_master` can be the current master for a
> `drm_device`. … Most of the modern IOCTL which require `DRM_MASTER`
> are for kernel modesetting - the current master is assumed to own
> the non-shareable display hardware.

Weston headless GL init (`libweston/backend-headless/headless.c`)
uses the same surfaceless platform as PipeWire, with
`egl_native_display = NULL` — no `card*` fd:

```c
const struct gl_renderer_display_options options = {
    .egl_platform = EGL_PLATFORM_SURFACELESS_MESA,
    .egl_native_display = NULL,
    ...
};
```

The DRM backend is the one that opens the KMS node and uses GBM
(`libweston/backend-drm/drm-gbm.c`):

```c
.egl_platform = EGL_PLATFORM_GBM_KHR,
.egl_native_display = b->gbm,
```

Khronos `EGL_EXT_device_drm`
<https://registry.khronos.org/EGL/extensions/EXT/EGL_EXT_device_drm.txt>:

> `EGL_DRM_MASTER_FD_EXT` … file descriptor with DRM master
> permissions on the DRM device …
> DRM master permissions are only required when EGL must modify
> output attributes. This extension does not define any situations
> in which output attributes will be modified.

Headless OpenGL/Vulkan composition does not modify KMS output
attributes, so a master fd is not required by this extension.

### Implication for SplitDesk Phase 0

| approach | DRM master? | Use in Phase 0 |
| --- | --- | --- |
| `-Bpipewire --renderer=gl` | no | **default** |
| `-Bheadless --renderer=gl` | no | fallback if PipeWire is absent; capture then needs another API (`weston-output-capture` / `--debug`, not a PipeWire node) |
| `-Bdrm` / `-Bdrm,pipewire` | **yes**, seat0 or `--seat` | forbidden for extra sessions |
| extra GPU + udev `ID_SEAT` + `-Bdrm --seat=…` | yes, on that card | only if a whole unused GPU is dedicated |
| DRM leasing | splits KMS objects under one owner | not a multi-user session model; skip |

Session processes need membership in the `render` group (or ACL) on
`/dev/dri/renderD*`. They must not be granted master on `/dev/dri/card*`.

`nvidia-drm.modeset=1` is **not** a way to share master. It enables
the NVIDIA KMS driver so GBM/Wayland on the *seat* compositor can
modeset. Leave modeset to the seat compositor; sessions stay on
render nodes.

### Risks

- Opening `/dev/dri/card0` from a session Weston (or from GBM
  `gbm_create_device(card0)`) can fail or steal master from the seat
  compositor. Do not do this.
- NVIDIA README 580.142 still says “Buffer allocation and submission
  to DRM KMS using gbm is not currently supported.” That sentence is
  about **KMS presentation**, not render-node GLES. Do not use
  Weston's DRM/GBM backend on NVIDIA as the multi-session path.
- `headless` with `WESTON_RENDERER_AUTO` becomes the **noop**
  renderer, not GL. Always pass `--renderer=gl`.
- Many concurrent GL compositors share one GPU. Kernel render nodes
  allow it; they do not guarantee latency isolation. Phase 0 should
  surface GPU contention in `diagnostics gpu`, not pretend exclusive
  access.

---

## Q4. How does NVIDIA provide headless OpenGL / Vulkan? (`EGL_EXT_platform_device`, GBM, `nvidia-drm`)

### Verdict: PARTIAL

NVIDIA documents several real headless/bootstrap paths. Which one
Weston actually calls (`EGL_MESA_platform_surfaceless`) is listed for
**Jetson** EGL, not in the **desktop** Linux README. Desktop README
580.142 still carries a stale “GBM+KMS not supported” line next to a
whole chapter on the GBM compositor backend. Phase 0 must probe the
host instead of assuming one NVIDIA path.

### Evidence

#### A. `EGL_EXT_platform_device` (Khronos, NVIDIA contact)

<https://registry.khronos.org/EGL/extensions/EXT/EGL_EXT_platform_device.txt>
(Version 6, 2014-05-16, James Jones / NVIDIA):

> Increasingly, EGL and its client APIs are being used in place of
> “native” rendering APIs … demand for a method to initialize EGL
> displays and surfaces directly on top of native GPU or device
> objects rather than native window system objects. …
> This extension defines a method to create an EGLDisplay from an
> EGLDeviceEXT …
> `EGL_PLATFORM_DEVICE_EXT` … `<native_display>` must be an
> `EGLDeviceEXT` object.
>
> RESOLVED: This extension defines no method to create window or
> pixmap surfaces on the EGLDeviceEXT platform. … EGLDeviceEXT-backed
> displays could expose EGLConfigs that only support rendering to
> EGLStreamKHR or EGLPbuffer surfaces.

#### B. `EGL_EXT_device_drm` — map device ↔ `/dev/dri/cardN`

<https://registry.khronos.org/EGL/extensions/EXT/EGL_EXT_device_drm.txt>:

> To obtain a DRM device file for an EGLDeviceEXT, call
> `eglQueryDeviceStringEXT` with `<name>` set to
> `EGL_DRM_DEVICE_FILE_EXT`. The function will return … the name of
> the device file (e.g. “/dev/dri/cardN”).
>
> If no file descriptor is specified and EGL requires one, it will
> attempt to open the device itself. … DRM master permissions are
> only required when EGL must modify output attributes.

#### C. NVIDIA Jetson: EGLDevice walk-through + extension list

EGLDevice chapter
<https://docs.nvidia.com/jetson/archives/r36.5.2/DeveloperGuide/SD/Graphics/Egldevice.html>
(updated 2026-08-03):

> EGLDevice provides a mechanism to access graphics functionality in
> the absence of or without reference to a native window system. …
> Create an EGLDisplay from an EGLDevice. … Query available
> EGLDevices with `eglQueryDevicesEXT()`. Obtain an EGLDisplay from
> the EGLDevice with `eglGetPlatformDisplayEXT()`. This step creates
> an EGLDisplay that does not belong to any native platform.

Listed client extensions on the same release
<https://docs.nvidia.com/jetson/archives/r36.5.2/DeveloperGuide/SD/Graphics/GraphicsAPIs.html>:

> `EGL_EXT_platform_device` `EGL_MESA_platform_surfaceless`
> `EGL_KHR_platform_gbm` `EGL_MESA_platform_gbm`
> `EGL_KHR_platform_wayland` …
>
> Vulkan … supports VK1.3

That is the official NVIDIA list that includes the platform Weston
pipewire/headless actually request.

#### D. Desktop README 580.142 — `nvidia-drm` + GBM + KMS

DRM KMS
<https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/kms.html>:

> The NVIDIA GPU driver package provides a kernel module,
> `nvidia-drm.ko`, which registers a DRM driver …
> Atomic Modeset: This is used for display of non-X11 based desktop
> environments, such as Wayland …
> NVIDIA's DRM KMS support is still considered experimental. It is
> disabled by default, but can be enabled … `modprobe nvidia_drm
> modeset=1`
>
> Applications can present through NVIDIA's DRM KMS implementation
> using any of the following:
>
> * The DRM KMS “dumb buffer” mechanism …
> * Using the `EGL_EXT_device_drm`, `EGL_EXT_output_drm`, and
>   `EGL_EXT_stream_consumer_egloutput` EGL extensions to associate
>   EGLStream producers with specific DRM KMS planes.
>
> Known Issues: … Buffer allocation and submission to DRM KMS using
> gbm is not currently supported.

GBM / Wayland compositors
<https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/gbm.html>:

> Most Wayland compositors use the GBM API to initialize an
> EGLDisplay object directly on a GPU, and to allocate the EGLSurface
> representing the desktop. The NVIDIA driver includes a GBM backend
> enabling the use of such software on NVIDIA GPUs.
>
> Requirements: DRM KMS must be enabled. … `libgbm.so.1` from Mesa
> version 21.2 … `egl-wayland` version 1.1.8 or later

Khronos `EGL_KHR_platform_gbm`
<https://registry.khronos.org/EGL/extensions/KHR/EGL_KHR_platform_gbm.txt>:

> To obtain an EGLDisplay from an GBM device, call
> `eglGetPlatformDisplay` with `<platform>` set to
> `EGL_PLATFORM_GBM_KHR`. The `<native_display>` parameter specifies
> the GBM device …

Weston's DRM backend is this path. Weston's PipeWire/headless
backends are **not**.

#### E. Mesa surfaceless platform (what Weston pipewire/headless call)

<https://registry.khronos.org/EGL/extensions/MESA/EGL_MESA_platform_surfaceless.txt>:

> This extension defines a new EGL platform, the “surfaceless”
> platform. This platform's defining property is that it has no
> native surfaces … The platform is independent of any native window
> system. The platform's intended use case is for enabling OpenGL and
> OpenGL ES applications on systems where no window system exists.
>
> `EGL_PLATFORM_SURFACELESS_MESA` … `<native_display>` parameter must
> be `EGL_DEFAULT_DISPLAY`.

#### F. Persistence for unused devices

<https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/nvidia-persistenced.html>:

> Whenever the NVIDIA device resources are no longer in use, the
> NVIDIA kernel driver will tear down the device state. …
> `nvidia-persistenced` … holds the NVIDIA character device files
> open … intended … for compute-only platforms where the NVIDIA
> device is not used to display a graphical user interface.

Useful on a headless GPU box; not a compositor API.

### Implication for SplitDesk Phase 0

Documented NVIDIA mechanisms, mapped to SplitDesk:

| mechanism | Who uses it | SplitDesk Phase 0 |
| --- | --- | --- |
| `EGL_MESA_platform_surfaceless` + render node | Weston `-Bpipewire/--renderer=gl` and `-Bheadless/--renderer=gl` | **primary**. Probe `EGL_EXTENSIONS` on `EGL_NO_DISPLAY` for `EGL_MESA_platform_surfaceless`. |
| `EGL_EXT_platform_device` + pbuffer/EGLStream | NVIDIA Jetson/DRIVE EGLDevice samples | **not** used by upstream Weston. Do not claim Weston is an EGLDevice compositor. |
| GBM + `EGL_KHR_platform_gbm` + `nvidia-drm.modeset=1` | Weston `-Bdrm`, seat compositor | seat0 only. Requires modeset. Conflicts with “do not take DRM master”. |
| EGLStream + `EGL_EXT_stream_consumer_egloutput` | NVIDIA KMS present path | display output, not multi-session capture. |
| Vulkan 1.3 ICD | Weston `--renderer=vulkan` (pipewire/headless have a Vulkan branch) | optional; treat as experimental vs `gl`. |
| `nvidia-persistenced` | driver daemon | optional on compute-only hosts so the first session is not a multi-second init. |

Capabilities probe (`nvidia`, `nvenc`, `wayland`, `pipewire`) should
record:

1. `nvidia-drm` loaded? `modeset` on or off? (modeset is for the
   *seat* compositor, not for session Weston.)
2. Presence of `/dev/dri/renderD*` for the NVIDIA device.
3. EGL client extensions: `EGL_MESA_platform_surfaceless`,
   `EGL_EXT_platform_device`, `EGL_KHR_platform_gbm`,
   `EGL_EXT_device_drm`.
4. Whether `weston -Bpipewire --renderer=gl` actually creates a GL
   renderer (log line `Using rendering device: /dev/dri/renderD…`
   from `gl_renderer_set_egl_device()`).

If surfaceless GL init fails, fall back to `--renderer=pixman` and
set `EncoderKind::SoftwareFallback` / `CaptureKind` still PipeWire,
with an explicit CPU-copy warning. Do not invent an EGLDevice
backend inside SplitDesk.

### Risks

- Desktop README 580.142 KMS chapter vs GBM chapter disagree on GBM.
  Quote both; do not resolve the contradiction in product copy.
- Jetson extension lists are not a guarantee for GeForce/workstation
  drivers. Probe.
- `nvidia-drm.modeset=1` on a machine whose seat compositor is Xorg
  without NVIDIA KMS can break the local desktop. SplitDesk must not
  flip this module parameter at session start.
- EGLDevice + EGLStream is NVIDIA's *on-screen* embedded path. Using
  it from a session would need DRM master / CRTC setup (Jetson
  “Setting Up the Display with DRM”). That fights Q3.
- Wayland known issues
  <https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/wayland-issues.html>
  still list missing workstation features and explicit-sync kernel
  requirements (≥ 6.8 for the listed syncobj fixes). Xwayland on
  NVIDIA needs `egl-wayland` ≥ 1.1.7/1.1.9 and DRM KMS.
- NVENC is not specified in any of the above compositor docs.
  Feeding NVENC is a SplitDesk media problem (dma-buf import), not a
  Weston/NVIDIA-README guarantee.

---

## Phase 0 spawn cheat-sheet

```
# as user U (systemd-run --uid=U --user if the daemon is root)
export XDG_RUNTIME_DIR=/run/user/$(id -u)
unset WAYLAND_DISPLAY
export XDG_SESSION_TYPE=wayland

# recommended
weston -Bpipewire --renderer=gl \
       --socket=splitdesk-sd-001 \
       --width=1920 --height=1080 \
       --idle-time=0 \
       --config=/path/to/session-weston.ini

# session-weston.ini
# [core]
# require-input=false
# [output]
# name=pipewire
# mode=1920x1080@60

# fallback if PipeWire is down
weston -Bheadless --renderer=gl \
       --socket=splitdesk-sd-001 \
       --width=1920 --height=1080 \
       --idle-time=0
```

Forbidden: `-Bdrm`, `--seat=seat0`, running Weston as root, writing a
world-writable `XDG_RUNTIME_DIR`, inheriting the seat
`WAYLAND_DISPLAY`.

---

## Sources (canonical)

| source | URL |
| --- | --- |
| Weston docs 16.0.90 *Running Weston* | https://wayland.pages.freedesktop.org/weston/toc/running-weston.html |
| Weston repo (backend-pipewire, backend-headless, frontend/main.c, man) | https://gitlab.freedesktop.org/wayland/weston |
| `weston(1)` 16.0.0 | https://manpages.debian.org/unstable/weston/weston.1.en.html |
| PipeWire streams 1.6.8 | https://docs.pipewire.org/page_streams.html |
| Linux DRM uAPI (master + render nodes) | https://docs.kernel.org/gpu/drm-uapi.html |
| EGL_EXT_platform_device | https://registry.khronos.org/EGL/extensions/EXT/EGL_EXT_platform_device.txt |
| EGL_EXT_device_drm | https://registry.khronos.org/EGL/extensions/EXT/EGL_EXT_device_drm.txt |
| EGL_MESA_platform_surfaceless | https://registry.khronos.org/EGL/extensions/MESA/EGL_MESA_platform_surfaceless.txt |
| EGL_KHR_platform_gbm | https://registry.khronos.org/EGL/extensions/KHR/EGL_KHR_platform_gbm.txt |
| NVIDIA README 580.142 DRM KMS | https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/kms.html |
| NVIDIA README 580.142 GBM | https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/gbm.html |
| NVIDIA README 580.142 persistenced | https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/nvidia-persistenced.html |
| NVIDIA README 580.142 Wayland issues | https://download.nvidia.com/XFree86/Linux-x86_64/580.142/README/wayland-issues.html |
| NVIDIA Jetson r36.5.2 EGLDevice | https://docs.nvidia.com/jetson/archives/r36.5.2/DeveloperGuide/SD/Graphics/Egldevice.html |
| NVIDIA Jetson r36.5.2 Graphics APIs | https://docs.nvidia.com/jetson/archives/r36.5.2/DeveloperGuide/SD/Graphics/GraphicsAPIs.html |
