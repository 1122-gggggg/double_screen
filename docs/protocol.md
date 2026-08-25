# Protocol

Two channels, not one:

1. **Daemon IPC** (local control plane): JSON-lines, `127.0.0.1:9823`, token file.
2. **Session protocol** (client ↔ session): length-prefixed JSON for control and reliable input; motion is latest-wins and may drop stale samples.

`PROTOCOL_VERSION` is `u16 = 1`.

Everything that crosses IPC is serialized with serde. Types live in `splitdesk-core` and `splitdesk-protocol`.

## Daemon IPC

Default bind: **`127.0.0.1:9823` only**. Not `0.0.0.0`.

Framing: one JSON object per line, UTF-8, `\n` terminated.

Authentication: `AuthToken` — 32 random bytes encoded as hex (64 hex chars), generated when the daemon starts, never hard-coded, never committed.

Token binding: `{ user, session_id, exp }`. A token for user A / session `sd-001` does not authorize user B or another session.

Token file, mode `0600` (owner read/write only):

| OS | Path |
| --- | --- |
| Linux | `$XDG_RUNTIME_DIR/splitdesk/daemon.token`, else `/tmp/splitdesk-$UID/daemon.token` |
| Windows | `%LOCALAPPDATA%\SplitDesk\daemon.token` |

CLI verbs (JSON-lines requests, same names):

```text
status
session list
session create --user <name>
session destroy
session info
metrics
diagnostics
diagnostics gpu
diagnostics media-path
```

`CreateSessionRequest`: `user`, `resolution` (default 1920×1080), `fps` (default 60), `memory_limit: Option<String>`, `cpu_affinity: Option<String>`.

## Session protocol framing

Control and reliable messages:

```text
u32 little-endian length || UTF-8 JSON payload
```

Length is the byte count of the JSON only, not including the four-byte prefix.

Motion (`PointerMotion`) may be sent on a drop-old path: keep the latest sample only. A sender or receiver with an unread motion sample **overwrites** it. Do not build a motion queue.

Buttons, keys, and control messages are reliable. They are not dropped for latency. Ordering of reliable messages is preserved.

## Handshake

Client → server:

```text
Hello {
  protocol_versions,   // list of u16; server picks 1 if present
  codecs,              // H264, Hevc, Av1 (subset the client can decode)
  input_caps,
  client_name
}
```

Server → client:

```text
HelloAck {
  protocol_version,    // 1
  session_id,          // Display sd-NNN
  capabilities,        // splitdesk-core::Capabilities
  selected_codec       // H264 preferred when NVENC is up
}
```

If the client does not offer version 1, the server errors `Protocol` / `Unsupported` and does not attach.

Codec preference: **H.264 first**. HEVC and AV1 are advertised in `Capabilities.codecs` when actually supported. Do not ACK a codec the encoder cannot produce.

## Control

`Control` messages:

`Status`, `SessionCreate`, `SessionDestroy`, `SessionList`, `SessionInfo`, `Metrics`, `Diagnostics`, `Attach`, `Detach`.

- `Attach` moves `Running` / `Detached` → `Connected`.
- `Detach` moves `Connected` → `Detached`. The compositor and apps keep running.
- `SessionDestroy` stops the session. Disconnect is not destroy.

## Input

| Message | Fields | Delivery |
| --- | --- | --- |
| `PointerMotion` | `x`, `y`, `ts` | Latest-wins; stale samples dropped |
| `PointerButton` | `button`, `pressed`, `ts` | Reliable |
| `Key` | `keycode`, `pressed`, `modifiers`, `ts` | Reliable |
| `Scroll` | `dx`, `dy`, `ts` | Latest-wins on the axis pair; do not unbounded-queue |

`ts` is the sender’s clock (`ClientLocal` for client-originated input). Do not treat it as `ServerLocal`.

## Cursor

```text
Cursor {
  shape_png_or_none,
  hotspot,
  visible,
  position
}
```

Shape may be omitted (`none`) when only position/visibility changed.

## Clipboard

```text
ClipboardUtf8 { text }
```

UTF-8 only in v1. **Never log the contents.** Logs may record size and direction, not the text.

## Disconnect

Server **must** release every virtual key and button it is holding for that client. A dropped TCP session is Disconnect. Stuck-key and stuck-button after a client crash is a protocol bug.

Detach without Disconnect (session remains) still releases keys/buttons held for that client.

## AuthToken rules

- Random 32-byte value, hex encoded.
- Bound to `{ user, session_id, exp }`.
- Reject expired, mismatched user, mismatched session, or missing token as `Auth`.
- Do not put tokens in source, tests as literals that work in production, container images, or world-readable files.
- `zeroize` on drop where the type is held in memory.

## Versioning

v1 is the only version. Unknown fields: reject (`Protocol`) rather than ignore if they change meaning of input or auth. Additive diagnostics fields may be ignored by older CLIs.
