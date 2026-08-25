# Benchmarking

Measure the path that actually ran. Do not publish a number without `MemoryPath`, clock domain, and codec/encoder kind.

There are **no** baked-in millisecond claims in this repository. `splitdesk-bench` is a harness, not a scoreboard.

## What to record

For every run:

- `HostOs`, GPU name, driver, `Capabilities.encoder`, `Capabilities.capture`, `selected_codec`
- `MemoryPath` (`render_output`, `capture`, `conversion`, `encoder_input`, `cpu_copies_per_frame`)
- Resolution, fps cap, whether the queue dropped frames (latest-wins)
- `LatencyStats` per stage: `mean`, `median`, `p95`, `p99`, `max`, `n`
- `ClockDomain` for every delta
- Network: loopback / LAN / WAN, RTT method

`splitdesk diagnostics media-path` and `splitdesk metrics` are the source of those fields. If `SoftwareFallback` ran, the log line for CPU-copy warning must exist for that run; a “GPU” label on a CPU-copy path is invalid.

## Stages

Thirteen stages, `T0ClientInput` … `T12Presented`:

| Stage | Where | Typical clock |
| --- | --- | --- |
| T0 `T0ClientInput` | Client produced a pointer/key event | `ClientLocal` |
| T1–T3 | Input send, server receive, inject into the session | mixed; see below |
| T4–T7 | Render, capture, convert, encode | `ServerLocal` |
| T8–T9 | Send / receive encoded frame | mixed |
| T10–T11 | Decode / compose | `ClientLocal` |
| T12 `T12Presented` | Frame on the client display | `ClientLocal` |

Exact middle variant names live in `splitdesk-metrics`. Docs do not invent extra public names.

## Clock domains

`ClockDomain`: `ClientLocal`, `ServerLocal`, `NetworkRtt`, `EstimatedOneWay`.

Rules:

- Deltas entirely on the client use `ClientLocal`. Entirely on the server use `ServerLocal`.
- **Never** subtract a client monotonic clock from a server monotonic clock without a measured offset. Those clocks are not one timeline.
- Offset from RTT/2 is `EstimatedOneWay` and **must be marked an estimate**.
- `NetworkRtt` is the round trip, not one-way latency.
- `LatencyStats.n` is the sample count after latest-frame drops. Do not count overwritten motion samples as delivered frames.

## Latest-frame-wins

Capture and motion keep depth 1. A stall in encode **drops** old frames rather than growing latency. Benchmarks must report drop count. A run that disabled dropping to make p99 look smooth is not a SplitDesk run.

## Invalid benchmarks

- GPU tests on CPU CI runners, fake `/dev/nvidia*`, mocked NVENC that copies a JPEG
- Comparing T0 and T12 across machines using raw `Instant::now()` on each side
- Quoting encode time only and calling it “end-to-end”
- Hiding `cpu_copies_per_frame > 0` behind “zero-copy”
- Unbounded queues, VNC, or xdotool in the path
- Windows multi-session numbers on Windows 10/11

## How to run (when binaries exist)

```text
cargo build -p splitdesk-bench --release
splitdesk diagnostics gpu
splitdesk diagnostics media-path
splitdesk metrics
```

GPU probes that fail must print `Unavailable` / `SoftwareFallback`, not a substitute device. `scripts/diagnostics-gpu.sh` is the Linux fact probe; it does not create GPU nodes.

CI (see `.github/workflows/ci.yml`) runs `cargo test` without `--ignored`. Tests that need a GPU are `#[ignore = "requires-gpu"]`.
