# SplitDesk

Low-latency multi-user GPU remote desktop. Applications and files stay on the host. The client is a display, input, and clipboard surface — not a copy of the user’s data.

Latency over quality. Latest-frame-wins. Bounded queues (depth 1 for motion and captured frames). No unbounded work queues on the media path.

## Status

Version `0.1.0`. Not a finished multi-user GPU product.

| Path | Honest state |
| --- | --- |
| Windows 10/11 host, one interactive session | **Usable loopback:** DXGI Desktop Duplication → CPU BGRA readback (`cpu_copies=1`) → SDFR on `127.0.0.1:9824` → native minifb client |
| Windows second interactive session | `MultiUserNotSupportedByHostOs` (no RDS bypass) |
| Linux multi-user Weston / PipeWire / NVENC | Code present, **not proven** on this tree’s CI host |
| H.264 NVENC live encode | Capability probe only; live path is BGRA SDFR, not NVENC |
| End-to-end latency p50/p95 | **NOT MEASURED** |

What this is **not**:

- Not a VNC/RDP clone and not a Termsrv/RDS license workaround.
- Not zero-copy. The working Windows path is D3D11 capture then one CPU Map/readback.
- Not multi-user on Windows 10/11.
- Not a CI-simulated GPU. Tests that need a GPU are `#[ignore = "requires-gpu"]`.

## Workspace

Rust edition 2021, MSRV 1.80, MIT license.

| Crate | Role |
| --- | --- |
| `splitdesk-core` | Shared types (`SessionId`, `Capabilities`, `Error`, `MemoryPath`, …) |
| `splitdesk-protocol` | Hello/control/input/clipboard, auth token, framing |
| `splitdesk-transport` | Session transport |
| `splitdesk-media` | Capture / encode / input / clipboard / audio backends |
| `splitdesk-session` | In-memory `SessionManager`, isolation rules |
| `splitdesk-metrics` | T0–T12 stages, clock domains, latency stats |
| `splitdesk-host-linux` | Weston headless sessions as the session UID |
| `splitdesk-host-windows` | Single interactive session; no Win10/11 multi-user |
| `splitdesk-client-core` | Shared client logic |
| `splitdesk-client-linux` / `splitdesk-client-windows` | Native clients |
| `splitdeskd` | Host daemon (control plane) |
| `splitdesk-cli` | `splitdesk` CLI |
| `splitdesk-bench` | Measurement harness — not a source of fake numbers |

Linux host code is compiled on Windows behind `cfg` and returns `BackendUnavailable` for live spawn. Windows host code is compiled on Linux the same way.

## Build

```text
rustc 1.80+
cargo build --workspace
cargo build --workspace --release
```

No GPU is required to **compile**. A GPU is required to **encode/capture** on the non-fallback path.

```text
cargo run -p splitdeskd
cargo run -p splitdesk-cli -- status
```

Default daemon bind is `127.0.0.1:9823` only. Never `0.0.0.0` by default.

## Windows loopback (working)

```text
cargo run -p splitdeskd
cargo run -p splitdesk-cli -- session create --user %USERNAME%
cargo run -p splitdesk-client-windows -- --user %USERNAME% --session sd-001
```

Control plane: `127.0.0.1:9823` (token in `%LOCALAPPDATA%\\SplitDesk\\daemon.token`).
Media plane: `127.0.0.1:9824` after `MediaHello`. Frames are SDFR BGRA, latest-wins.
SDFR framing uses shared frame storage, vectored socket writes, and zero-copy client buffer splits;
it does not add full-payload copies beyond the documented capture/readback path.
A second `session create` on Windows 10/11 fails with `MultiUserNotSupportedByHostOs`.

## CLI

```text
splitdesk status
splitdesk session list
splitdesk session create --user <name>
splitdesk session destroy
splitdesk session info
splitdesk metrics
splitdesk diagnostics
splitdesk diagnostics gpu
splitdesk diagnostics media-path
```

Daemon IPC is JSON-lines, authenticated with a token generated at daemon start (never hard-coded):

- Linux: `$XDG_RUNTIME_DIR/splitdesk/daemon.token`, else `/tmp/splitdesk-$UID/daemon.token`, mode `0600`
- Windows: `%LOCALAPPDATA%\SplitDesk\daemon.token`, mode equivalent to owner-only

## Host rules (short)

- **Linux:** look up the user, set `WAYLAND_DISPLAY=splitdesk-<id>`, spawn Weston as that UID (not root). If the daemon is root, `systemd-run --uid=…`. If it is not root, only the current user. Do not take seat0 DRM master. Prefer headless / EGLDevice.
- **Windows 10/11:** at most one interactive session. Windows Server RDS is a separate `SessionSupport` value, not a back door on workstation SKUs.

## Prohibited shortcuts

Do not implement or regress to:

- VNC (or wrapping libvncserver / TightVNC / TigerVNC as the session)
- `xdotool` as input
- global `uinput` as the **primary** input path
- Electron as the host compositor or client shell
- `termsrv.dll` patches, RDP wrapper, or other RDS-license bypasses
- plaintext password database
- hard-coded auth tokens
- `chmod 777` on tokens, sockets, or runtime dirs
- running the compositor (Weston / DWM stand-in) as root

## Docs

- [Architecture](docs/architecture.md)
- [Protocol](docs/protocol.md)
- [Linux host](docs/linux-host.md)
- [Windows host](docs/windows-host.md)
- [Security](docs/security.md)
- [Benchmarking](docs/benchmarking.md)
- [MVP acceptance](tests/mvp-acceptance.md)

Research notes under `docs/research/` are investigation, not promises.

## License

MIT. See [LICENSE](LICENSE).
