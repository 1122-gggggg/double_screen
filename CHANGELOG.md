# Changelog

All notable changes to SplitDesk are documented here. Versions follow Semantic Versioning.

## [0.2.0] - 2026-08-30

### Added

- One `splitdesk-client` binary for Linux, Windows, and macOS.
- macOS and unknown-host capability reporting without compile-time daemon failure.
- Independent loopback media endpoint configuration for IPv4, IPv6, tests, and SSH tunnels.
- Native CI and release builds for Ubuntu, Windows, and macOS, with SHA-256 checksums.

### Changed

- Linux and Windows legacy client binaries now delegate to the shared client implementation.
- Client input transport is bounded and retries the latest unsent input under backpressure.
- Repository metadata and Linux service documentation point to the canonical repository.
- The Weston input module adapts to the versioned 13–16 input event ABI and treats external
  libweston headers as system headers while retaining warnings-as-errors for SplitDesk code.

### Security

- Token replacement is written through a same-directory temporary file and installed with the
  platform replacement primitive.
- Unix token files reject symlink destinations and retain mode `0600`.
- Windows token replacement uses `MoveFileExW` with replace and write-through flags.
- Control and media listeners both reject non-loopback bind addresses.
- Control and media admission cap concurrent connections, time out unauthenticated/idle peers,
  and reject JSON lines over 8 KiB before parsing.
- OpenH264 was upgraded to 0.8.1 to remove `RUSTSEC-2025-0008` from the decoder path.
- MSRV is now Rust 1.85, matching the pinned toolchain and the patched decoder dependency.

### Known limitations

- Linux and Windows are host platforms; macOS is currently a native client platform only.
- Remote use requires an SSH tunnel. WebRTC data transport remains capability negotiation only.
- Release archives are checksummed but not code-signed.
