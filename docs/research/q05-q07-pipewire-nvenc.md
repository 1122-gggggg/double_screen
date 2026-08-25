# SplitDesk research Q5–Q7: PipeWire DMA-BUF, GStreamer NVENC memory, Weston input

Official-docs note for the Linux media path. Verdicts are `SUPPORTED`, `PARTIAL`, `UNSUPPORTED`, or `UNKNOWN`. Every claim below is grounded in a cited official page or first-party header. Versions observed while fetching:

| Source | Version on the page |
| --- | --- |
| PipeWire API (`docs.pipewire.org`) | 1.6.8 |
| GStreamer nvcodec / nvh264enc | current published plugin docs (Gst Bad Plug-ins; `nvh264enc` “Encode H.264 video streams using NVCODEC API CUDA Mode”) |
| NVIDIA Video Codec SDK | 13.0 |
| Weston / libweston | 16.0.90 |
| libei / libeis | 1.6.0 |

Do not treat this note as a zero-copy guarantee. `SPA_DATA_DmaBuf` and `memory:DMABuf` mean “fd-backed DMA-BUF”, not “this buffer is already a CUDA surface NVENC can consume”.

---

## Q5. Can PipeWire frames stay DMA-BUF / GPU-resident into GStreamer?

**Verdict: PARTIAL**

PipeWire can carry video as `SPA_DATA_DmaBuf` (an fd that is *not* assumed CPU-mappable). The official PipeWire GStreamer plugin will wrap that fd with GStreamer’s DMA-BUF allocator and default to *not* copying (`DEFAULT_ALWAYS_COPY false`). GStreamer itself documents `memory:DMABuf` as the first-class caps feature for those fds.

That is **not** the same as staying GPU-resident all the way to NVENC. `nvh264enc` does not advertise `memory:DMABuf` on its sink (see Q6). Official `cudaupload` also does not list `memory:DMABuf` on its sink. A DMA-BUF that arrives in GStreamer must still be imported into `CUDAMemory` or `GLMemory` (or copied to system memory) before `nvh264enc`. Mapping a DMA-BUF on the CPU is officially the slow / sometimes-impossible path.

### Official quotes

PipeWire design — fd passing is the raw-video efficiency path:

> PipeWire was designed to: Be efficient for raw video using fd passing and audio with shared ringbuffers.

Source: <https://docs.pipewire.org/page_design.html>

PipeWire data type — DMA-BUF is fd-backed and normally *not* mmap’d:

> `SPA_DATA_DmaBuf` — fd to dmabuf memory. This might not be readily mappable (unless the MAPPABLE flag is set) and should normally be handled with DMABUF apis.

Source: <https://docs.pipewire.org/group__spa__buffer.html>

`spa_data` is the per-plane descriptor (`type`, `fd`, `mapoffset`, `maxsize`, optional `data` pointer):

> Data for a buffer this stays constant for a buffer. … `type` — memory type, one of enum `spa_data_type` … `fd` — optional fd for data.

Source: <https://docs.pipewire.org/structspa__data.html>

GStreamer DMA-BUF design — those fds become `memory:DMABuf` `GstBuffer`s:

> The DMA buffer sharing is the efficient way to share the buffer/memory between different Linux kernel driver, such as codecs/3D/display/cameras. For example, the decoder may want its output to be directly shared with the display server for rendering without a copy. … This kind of buffer/memory is usually stored in non-system memory (maybe in device's local memory or something else not directly accessible by the CPU), then its memory mapping for CPU access may impose a big overhead and low performance, or even impossible. … The *GstCapsFeatures* *memory:DMABuf* is usually used to negotiate DMA buffers.

Source: <https://gstreamer.freedesktop.org/documentation/additional/design/dmabuf.html>

Official PipeWire `pipewiresrc` (first-party source; GStreamer.org has no published `pipewiresrc` page — `https://gstreamer.freedesktop.org/documentation/pipewire/pipewiresrc.html` returned HTTP 404) wraps `SPA_DATA_DmaBuf` with `gst_dmabuf` and defaults to no copy:

```c
#include <gst/allocators/gstfdmemory.h>
#include <gst/allocators/gstdmabuf.h>
#define DEFAULT_ALWAYS_COPY     false
/* ... */
else if(d->type == SPA_DATA_DmaBuf) {
    /* ... */
    gmem = gst_fd_allocator_alloc (pool->dmabuf_allocator, dup(d->fd),
        d->mapoffset + d->maxsize, fd_flags);
```

Sources:

- <https://gitlab.freedesktop.org/pipewire/pipewire/-/raw/master/src/gst/gstpipewiresrc.c>
- <https://gitlab.freedesktop.org/pipewire/pipewire/-/raw/master/src/gst/gstpipewirepool.c>

`cudaupload` sink templates (system / `GLMemory` / `D3D11Memory` / `CUDAMemory` — **no** `memory:DMABuf`):

> `video/x-raw:` … `video/x-raw(memory:GLMemory):` … `video/x-raw(memory:D3D11Memory):` … `video/x-raw(memory:CUDAMemory):`

Source: <https://gstreamer.freedesktop.org/documentation/nvcodec/cudaupload.html>

### SplitDesk implication

- Capture (`CaptureKind::PipeWire`) **must** negotiate `SPA_DATA_DmaBuf` + `SPA_FORMAT_VIDEO_modifier` with the Weston/PipeWire producer. Fallback is `SPA_DATA_MemFd` / `SPA_DATA_MemPtr` → `MemoryType::SystemMemory` and `cpu_copies_per_frame >= 1`.
- A DMA-BUF that lands in GStreamer is still `MemoryType::DmaBuf`, not `CudaMemory`. `diagnostics media-path` must print the conversion hop (`DmaBuf` → `GLMemory`/`CudaMemory`) honestly. Do not claim zero-copy into NVENC.
- Keep `pipewiresrc` `always-copy=false` (plugin default). Forcing a copy is the SoftwareFallback warning path.
- Latest-frame-wins: do **not** insert an unbounded `queue` after `pipewiresrc`. PipeWire already bounds buffers (`min-buffers` default 1). Depth-1 drop-old is the SplitDesk contract.

---

## Q6. Which GPU memory types does GStreamer NVENC (`nvcodec` / `nvh264enc`) accept?

**Verdict: PARTIAL** (of the four types asked)

`nvh264enc` is documented as **“Encode H.264 video streams using NVCODEC API CUDA Mode”**. Its official sink pad template lists four memory features. `memory:DMABuf` is **absent**.

### NVENC sink memory-type table (`nvh264enc`)

| Memory type asked | Official sink feature | Verdict | Notes |
| --- | --- | --- | --- |
| `CUDAMemory` | `video/x-raw(memory:CUDAMemory)` | **SUPPORTED** | First listed sink template. Matches the element’s CUDA Mode. Preferred SplitDesk `MemoryType::CudaMemory` encoder input. |
| `GLMemory` | `video/x-raw(memory:GLMemory)` | **SUPPORTED** | GPU-resident via GL; still not DMA-BUF. Useful if capture/import is GL. |
| `D3D12Memory` | `video/x-raw(memory:D3D12Memory)` | **SUPPORTED** | Windows-oriented. Not the Linux PipeWire path. NVIDIA NVENC also documents `NV_ENC_DEVICE_TYPE_DIRECTX` for D3D12. |
| `DMABuf` | *(not in pad template)* | **UNSUPPORTED** | Cannot feed `memory:DMABuf` directly into `nvh264enc`. Must convert. |
| system `video/x-raw` | `video/x-raw` (no memory feature) | **SUPPORTED** (CPU path) | Triggers an upload. This is `MemoryType::SystemMemory` + `cpu_copies_per_frame >= 1`. SoftwareFallback must log the CPU-copy warning and measure copy latency. |

Official sink pad template (verbatim structure from the published page):

```
video/x-raw(memory:CUDAMemory):
         format: { NV12, Y444, VUYA, RGBA, RGBx, BGRA, BGRx }
          width: [ 160, 4096 ]
         height: [ 64, 4096 ]
 interlace-mode: progressive

video/x-raw(memory:D3D12Memory):
         format: { NV12, Y444, VUYA, RGBA, RGBx, BGRA, BGRx }
          ...

video/x-raw(memory:GLMemory):
         format: { NV12, Y444, VUYA, RGBA, RGBx, BGRA, BGRx }
          ...
video/x-raw:
         format: { NV12, Y444, VUYA, RGBA, RGBx, BGRA, BGRx }
          ...
```

Source: <https://gstreamer.freedesktop.org/documentation/nvcodec/nvh264enc.html>

The nvcodec plugin index confirms CUDA helpers around the encoder (`cudaupload` / `cudadownload` / `cudaconvert` / `cudascale`) and names `nvh264enc` “NVCODEC API CUDA Mode”. Direct3D11-mode encoders are separate elements (`nvd3d11h264enc`), not `nvh264enc`.

Source: <https://gstreamer.freedesktop.org/documentation/nvcodec/>

NVIDIA Video Codec SDK 13.0 — devices NVENC actually binds (no Linux DMA-BUF device type):

> The NVIDIA Encoder supports use of the following types of devices: DirectX 9 / 10 / 11 / 12 … CUDA … OpenGL. … Use of CUDA device for Encoding is supported on Linux and Windows 10 and later … Use of the OpenGL device type for encoding is supported only on Linux.

Source: <https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html>

### Official low-latency knobs

From `nvh264enc` properties (same page as the pad template):

| Property | Official text | Default | SplitDesk setting |
| --- | --- | --- | --- |
| `bframes` | “Number of B-frames between I and P” | `0` | Keep `0`. Do not enable B-frames. |
| `zerolatency` | “Zero latency operation (no reordering delay)” | `false` | Set `true`. |
| `preset` | `GstNvEncoderPreset` — `p1` “P1, fastest” … `p7` “P7, slowest”. Legacy `low-latency` / `low-latency-hp` / `low-latency-hq` are **deprecated**: “use p1~7 with tune”. | `default (0)` | Prefer `p1` (latency) or `p3` if quality is unacceptable. |
| `tune` | `GstNvEncoderTune`: `low-latency (2)`, `ultra-low-latency (3)` | `default (0)` | `ultra-low-latency` first; `low-latency` if ULL quality is unusable. |
| `gop-size` | “Number of frames between intra frames (-1 = infinite)” | `75` | `-1` (infinite GOP) or a short IDR cadence; never rely on B-frame GOPs. |
| `rc-lookahead` | “Number of frames for frame type lookahead” | `0` | Keep `0`. Lookahead adds delay. |
| `b-adapt` | “Enable adaptive B-frame insert when lookahead is enabled” | `false` | Keep `false`. |
| `multi-pass` | `disabled` / `two-pass-quarter` / `two-pass` | `default (0)` | `disabled`. Multi-pass is extra latency. |
| `rc-mode` | `cbr (2)`, `vbr (3)`, … | `default (0)` | `cbr` for a constrained remote-desktop channel. |
| `vbv-buffer-size` | “VBV(HRD) Buffer Size in kbits (0 = NVENC default)” | `0` | Keep small (about one frame: bitrate/fps). Large VBV is extra delay. |

`GstNvEncoderPreset` / `GstNvEncoderTune` member list:

> `p1 (8)` – P1, fastest … `p7 (14)` – P7, slowest … `low-latency (2)` – Low latency … `ultra-low-latency (3)` – Ultra low latency

Source: <https://gstreamer.freedesktop.org/documentation/nvcodec/nvh265enc.html> (named constants shared with `nvh264enc`; the `nvh264enc` page links these enums here)

NVIDIA Video Codec SDK 13.0 tuning table — cloud gaming / conferencing maps to Low latency / Ultra-low latency + CBR:

> Cloud gaming / Streaming / Video conferencing **In high bandwidth channel** → Low latency, with CBR. … **In strictly bandwidth-constrained channel** → Ultra-low latency, with CBR. … For each tuning info, seven presets from P1 (highest performance) to P7 (lowest performance) …

Source: <https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html>

Legacy LowLatencyHP / LowLatencyDefault / LowLatencyHQ presets migrate to Tuning Info **Low Latency** or **Ultra Low Latency** + P1–P5 + CBR (SDK 10+ architecture):

Source: <https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-preset-migration-guide/index.html>

NVIDIA also notes B-frames cost extra IO buffers:

> The client should allocate at least (1 + NB) input and output buffers, where NB is the number of B frames between successive P frames.

Source: <https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html>

### SplitDesk implication

- Linux encoder input is `CUDAMemory` (or `GLMemory` as a documented alternate). `MemoryPath.encoder_input` is never `DmaBuf` when the encoder is `nvh264enc`.
- Conversion hop is mandatory: PipeWire `DmaBuf` → (GL import or CUDA import) → `nvh264enc`. If that import is missing, fall back to `SoftwareFallback` / system `video/x-raw`, log the CPU-copy warning, and fill `cpu_copies_per_frame`.
- Encode settings: `bframes=0 zerolatency=true tune=ultra-low-latency preset=p1 rc-lookahead=0 multi-pass=disabled rc-mode=cbr gop-size=-1`. Do not re-enable B-frames “for quality”.
- `nvd3d11h264enc` / `D3D12Memory` are Windows host-path concerns, not Linux PipeWire.

---

## Q7. How should Weston / libweston do per-session remote input (not uinput / xdotool)?

**Verdict: PARTIAL**

Official Weston documents that **the back-end owns input**. The PipeWire back-end is explicitly **output-only**. Headless is **no input**. The only stock remote-input back-end is **RDP**, which is not SplitDesk’s protocol and must not be used as a termsrv/VNC-style bypass.

The supported per-session path is therefore: keep one Weston compositor (and one `weston_seat`) inside the user’s session (`WAYLAND_DISPLAY=splitdesk-<id>`), and inject pointer/key/scroll into **that seat** with libweston’s public `weston_*_send_*` APIs (or a tiny custom back-end/plugin that calls them). That stays inside the compositor process. It does not create a global kernel device (`uinput`) and does not drive X11 (`xdotool`).

libei is the official Wayland-stack *emulated input* protocol (EIS in the compositor, EI client in the remote-desktop process). Weston 16.0.90’s published table of contents does **not** list a stock libei / EIS back-end. Treat libei as the standard compositor-side pointer if SplitDesk later grows an EIS plugin — not as something `weston -Bpipewire` already provides.

### Official quotes

Back-end owns input; available back-ends:

> libweston uses the concept of a *back-end* to abstract the interface to the underlying environment where it runs on. Ultimately, the back-end is responsible for handling the input and generate an output. … Available back-ends: **drm** – run stand-alone on DRM/KMS and evdev … **rdp** – run as an RDP server without local input or output … **headless** – run without input or output, useful for test suite … **pipewire** – run without input, output into a PipeWire node

Source: <https://wayland.pages.freedesktop.org/weston/toc/running-weston.html>

A seat is the isolation unit (one person’s devices). Stand-alone Weston uses `seat0` by default; `--seat` selects another. A graphics card is required to be part of a stand-alone seat. DRM can start with `--continue-without-input` / `require-input=false` if there are no physical devices.

Source: same page, “Running Weston on a stand-alone back-end” / “Running Weston on a different seat”.

libweston purpose:

> Libweston provides most of the boring and tedious bits of correctly implementing core Wayland protocols and interfacing with input and output systems.

Source: <https://wayland.pages.freedesktop.org/weston/toc/libweston.html>

Public per-seat injection / send APIs (first-party `libweston.h`):

```c
void weston_pointer_send_motion(struct weston_pointer *pointer,
                               const struct timespec *time,
                               struct weston_pointer_motion_event *event);
void weston_pointer_send_button(struct weston_pointer *pointer,
                               const struct timespec *time,
                               uint32_t button, uint32_t state_w);
void weston_pointer_send_axis(struct weston_pointer *pointer,
                             const struct timespec *time,
                             struct weston_pointer_axis_event *event);
void weston_pointer_send_frame(struct weston_pointer *pointer);
void weston_pointer_move(struct weston_pointer *pointer,
                         struct weston_pointer_motion_event *event);

void weston_keyboard_send_key(struct weston_keyboard *keyboard,
                             const struct timespec *time, uint32_t key,
                             enum wl_keyboard_key_state state);
void weston_keyboard_send_modifiers(struct weston_keyboard *keyboard,
                                   uint32_t serial, uint32_t mods_depressed,
                                   uint32_t mods_latched,
                                   uint32_t mods_locked, uint32_t group);

struct weston_pointer  *weston_seat_get_pointer(struct weston_seat *seat);
struct weston_keyboard *weston_seat_get_keyboard(struct weston_seat *seat);
```

`struct weston_seat` holds `pointer_state`, `keyboard_state`, `touch_state`, and `seat_name` — one seat per interactive user/session.

Source: <https://gitlab.freedesktop.org/wayland/weston/-/raw/master/include/libweston/libweston.h>

libweston compositor docs also expose `weston_compositor_set_default_pointer_grab` and `weston_compositor_set_wake_on_input` (input can be processed without auto-waking; custom grabs can define policy).

Source: <https://wayland.pages.freedesktop.org/weston/toc/libweston/compositor.html>

libei — official pointer only (not a Weston built-in):

> **libei** is a library for Emulated Input, primarily aimed at the Wayland stack. … the server side, typically a Wayland compositor, is called the **“EIS Implementation”**. … Note how the EI client is roughly equivalent to a physical input device coming from the kernel and its events feed into the normal input stack. However, the events are distinguishable inside the compositor … Events from the EIS implementation would usually feed into the input stack in the same way as input events from physical devices. To Wayland clients, they are indistinguishable from real devices.

> libeis is the server-side module. This API should be used by processes that have control over input devices, e.g. Wayland compositors. … create one or more seats for the client with `eis_client_new_seat()` … `EIS_EVENT_POINTER_MOTION` … `EIS_EVENT_POINTER_MOTION_ABSOLUTE` … `EIS_EVENT_BUTTON_BUTTON` … `EIS_EVENT_KEYBOARD_KEY` … `EIS_EVENT_SCROLL_DELTA`

Sources:

- <https://libinput.pages.freedesktop.org/libei/>
- <https://libinput.pages.freedesktop.org/libei/api/group__libeis.html>

Weston 16.0.90 published docs index lists Running Weston, Libweston (compositor / config / shell / output / logging), test suite, kiosk-shell, IVI-shell, content restrictions. No libei / EIS page.

Source: <https://wayland.pages.freedesktop.org/weston/>

### SplitDesk implication

- Spawn Weston **as the session UID** (`WAYLAND_DISPLAY=splitdesk-<id>`). Prefer headless/EGLDevice + PipeWire output. Do **not** take seat0 DRM master for a remote-only session.
- Input path: a libweston plugin / tiny custom back-end that maps SplitDesk `PointerMotion` / `PointerButton` / `Key` / `Scroll` onto `weston_pointer_send_*` and `weston_keyboard_send_key` for **that session’s** `weston_seat`. Motion is latest-wins; buttons and keys are reliable. On `Disconnect`, release every virtual key/button via `send_key`/`send_button` (compositor-local, not a global uinput reset).
- Isolation: different users never share `wayland_display` or the same `weston_seat`. A compositor-local seat cannot leak into another user’s Weston.
- Forbidden: global `uinput`, `xdotool`, injecting into seat0 of the login session, RDP/VNC as the SplitDesk transport.
- libei: optional later EIS plugin if input must come from a process *outside* Weston. Today’s official Weston docs do not ship that plugin; do not pretend `weston -Bpipewire` already accepts libei.

---

## Memory path SplitDesk should print (`diagnostics media-path`)

Linux NVENC happy path (no pretended zero-copy past the documented hop):

| Stage | `MemoryType` | Notes |
| --- | --- | --- |
| `render_output` | `DmaBuf` (or `GlMemory` if Weston GL renderer exports GL) | Weston PipeWire back-end is output-only. |
| `capture` | `DmaBuf` | PipeWire `SPA_DATA_DmaBuf` → GStreamer `memory:DMABuf`. |
| `conversion` | `GlMemory` or `CudaMemory` | Required. `nvh264enc` does not accept DMA-BUF. |
| `encoder_input` | `CudaMemory` (preferred) or `GlMemory` | `nvh264enc` CUDA Mode. |
| `cpu_copies_per_frame` | `0` on the happy path; `>= 1` on `MemFd`/`video/x-raw` fallback | SoftwareFallback logs CPU-copy warning + measured copy latency. |

Windows (out of this note’s host path, for the table only): `D3D12` / `D3D11` → `nvd3d11h264enc` or `nvh264enc` `D3D12Memory`. Not PipeWire.

---

## Sources (canonical)

1. <https://docs.pipewire.org> — overview, design, SPA buffers, `spa_data`, `SPA_DATA_DmaBuf`
2. <https://docs.pipewire.org/page_design.html>
3. <https://docs.pipewire.org/group__spa__buffer.html>
4. <https://docs.pipewire.org/structspa__data.html>
5. <https://gitlab.freedesktop.org/pipewire/pipewire/-/raw/master/src/gst/gstpipewiresrc.c>
6. <https://gitlab.freedesktop.org/pipewire/pipewire/-/raw/master/src/gst/gstpipewirepool.c>
7. <https://gstreamer.freedesktop.org/documentation/additional/design/dmabuf.html>
8. <https://gstreamer.freedesktop.org/documentation/nvcodec/>
9. <https://gstreamer.freedesktop.org/documentation/nvcodec/nvh264enc.html>
10. <https://gstreamer.freedesktop.org/documentation/nvcodec/nvh265enc.html> (`GstNvEncoderPreset` / `GstNvEncoderTune`)
11. <https://gstreamer.freedesktop.org/documentation/nvcodec/cudaupload.html>
12. <https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html>
13. <https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-preset-migration-guide/index.html>
14. <https://wayland.pages.freedesktop.org/weston/toc/running-weston.html>
15. <https://wayland.pages.freedesktop.org/weston/toc/libweston.html>
16. <https://wayland.pages.freedesktop.org/weston/toc/libweston/compositor.html>
17. <https://gitlab.freedesktop.org/wayland/weston/-/raw/master/include/libweston/libweston.h>
18. <https://libinput.pages.freedesktop.org/libei/> (Q7 pointer only)
19. <https://libinput.pages.freedesktop.org/libei/api/group__libeis.html> (Q7 pointer only)
