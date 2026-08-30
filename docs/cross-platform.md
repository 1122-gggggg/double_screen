# Cross-platform usage

SplitDesk 0.2 separates client portability from host capture support. The same native client
source is built on Linux, Windows, and macOS. Host capture remains platform-specific and reports
its capability honestly.

## Support matrix

| Operating system | Native client | CLI / diagnostics | Host sessions |
| --- | --- | --- | --- |
| Linux | Supported | Supported | Multi-user Weston + PipeWire + NVENC, hardware-gated |
| Windows 10/11 | Supported | Supported | One interactive DXGI session |
| macOS | Supported | Supported | Not implemented (`UnsupportedHost`) |

The macOS daemon compiles so tooling can query status and capabilities, but it cannot create a
desktop session. A macOS client can attach to a Linux or Windows host through an SSH tunnel.

## Encrypted remote connection

The current control and media protocols are authenticated but do not provide transport
encryption. Both daemon listeners therefore require loopback addresses. Do not change either
listener to a wildcard or LAN address.

Forward both planes from the client machine:

```text
ssh -N -L 19823:127.0.0.1:9823 -L 19824:127.0.0.1:9824 user@splitdesk-host
```

Then launch the native client in a second terminal:

```text
splitdesk-client --server 127.0.0.1:19823 --media-server 127.0.0.1:19824 --user <host-user> --token-file <local-token-file>
```

Copy `daemon.token` to the client only over an authenticated channel, set restrictive local file
permissions, and delete the copy after use. `--token` remains available for compatibility but can
expose the token in process arguments; prefer `--token-file`.

IPv6 loopback is supported with bracketed socket syntax, for example `[::1]:19823`.

## Daemon endpoints

Defaults:

- control: `127.0.0.1:9823`
- media: `127.0.0.1:9824`

Use `splitdeskd --bind <loopback:port> --media-bind <loopback:port>` to avoid port collisions or
to use IPv6. Port `0` is accepted for test harnesses and the daemon reports the actual bound media
endpoint to clients.

## Release artifacts

Tagged releases build archives on native Ubuntu, Windows, and macOS runners and publish a SHA-256
checksum beside each archive. Every archive contains `splitdesk`, `splitdesk-client`, and
`splitdeskd`; on macOS the daemon is diagnostics-only and rejects session creation. These artifacts
are not code-signed. Production distribution still needs platform signing and notarization.

## Not yet cross-platform

- macOS host capture/input (ScreenCaptureKit, VideoToolbox, and a permission-aware input adapter)
- encrypted built-in remote transport and NAT traversal
- signed installers and automatic updates
- hardware-backed GPU acceptance in public CI
