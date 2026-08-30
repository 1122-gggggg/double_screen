use std::collections::HashMap;
use std::io::{Error as IoError, ErrorKind, IoSlice};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use splitdesk_core::SessionId;
use splitdesk_media::BoundedSlot;
use splitdesk_protocol::{encode_media_frame_header, Input, MediaFrame, MediaHello, MediaHelloAck};
use splitdesk_session::SessionManager;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::sync::Semaphore;

pub(crate) const MAX_JSON_LINE_BYTES: usize = 8 * 1024;
const MAX_MEDIA_CONNECTIONS: usize = 64;
const PRE_AUTH_TIMEOUT: Duration = Duration::from_secs(5);

use crate::auth::authorize;
use crate::token::Token;

pub(crate) struct SessionFrameSlot {
    latest: BoundedSlot<MediaFrame>,
    ready: Notify,
    live_capture: AtomicBool,
    input: Option<SessionInputEndpoint>,
}

#[derive(Clone)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct SessionInputEndpoint {
    pub socket: PathBuf,
    pub uid: u32,
}

#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
impl SessionFrameSlot {
    fn new(input: Option<SessionInputEndpoint>) -> Self {
        Self {
            latest: BoundedSlot::new(),
            ready: Notify::new(),
            live_capture: AtomicBool::new(false),
            input,
        }
    }

    pub(crate) fn push(&self, frame: MediaFrame) {
        self.latest.push(frame);
        self.ready.notify_one();
    }

    pub(crate) fn set_live_capture(&self, live: bool) {
        self.live_capture.store(live, Ordering::Release);
    }

    async fn take_latest(&self) -> MediaFrame {
        loop {
            let notified = self.ready.notified();
            if let Some(frame) = self.latest.take() {
                return frame;
            }
            notified.await;
        }
    }
}

pub(crate) struct FrameHub {
    slots: Mutex<HashMap<SessionId, Arc<SessionFrameSlot>>>,
}

impl FrameHub {
    pub(crate) fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }

    #[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
    pub(crate) fn register(
        &self,
        id: SessionId,
        input: Option<SessionInputEndpoint>,
    ) -> Arc<SessionFrameSlot> {
        let slot = Arc::new(SessionFrameSlot::new(input));
        self.slots.lock().insert(id, Arc::clone(&slot));
        slot
    }

    pub(crate) fn remove(&self, id: &SessionId) {
        self.slots.lock().remove(id);
    }

    fn get(&self, id: &SessionId) -> Option<Arc<SessionFrameSlot>> {
        self.slots.lock().get(id).cloned()
    }

    pub(crate) fn has_live_capture(&self) -> bool {
        self.slots
            .lock()
            .values()
            .any(|slot| slot.live_capture.load(Ordering::Acquire))
    }
}

pub(crate) async fn serve_media(
    listener: TcpListener,
    token: Arc<Token>,
    manager: Arc<SessionManager>,
    frames: Arc<FrameHub>,
) {
    let permits = Arc::new(Semaphore::new(MAX_MEDIA_CONNECTIONS));
    loop {
        match listener.accept().await {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    tracing::warn!(peer = %peer, limit = MAX_MEDIA_CONNECTIONS, "media connection limit reached");
                    continue;
                };
                let token = Arc::clone(&token);
                let manager = Arc::clone(&manager);
                let frames = Arc::clone(&frames);
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(err) = handle_media_connection(stream, token, manager, frames).await
                    {
                        tracing::warn!(peer = %peer, error = %err, "media connection closed");
                    }
                });
            }
            Ok((_stream, peer)) => {
                tracing::warn!(peer = %peer, "rejected non-loopback media connection");
            }
            Err(err) => tracing::error!(error = %err, "media accept failed"),
        }
    }
}

async fn handle_media_connection(
    stream: TcpStream,
    token: Arc<Token>,
    manager: Arc<SessionManager>,
    frames: Arc<FrameHub>,
) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let Some(line) = tokio::time::timeout(
        PRE_AUTH_TIMEOUT,
        read_bounded_line(&mut reader, MAX_JSON_LINE_BYTES),
    )
    .await??
    else {
        return Ok(());
    };
    let hello: MediaHello = match serde_json::from_str(&line) {
        Ok(hello) => hello,
        Err(_) => {
            write_ack(
                &mut writer,
                MediaHelloAck {
                    ok: false,
                    code: Some("Protocol".into()),
                    width: None,
                    height: None,
                },
            )
            .await?;
            return Ok(());
        }
    };
    if let Err(failure) = authorize(&Some(hello.token), token.as_str()) {
        write_ack(
            &mut writer,
            MediaHelloAck {
                ok: false,
                code: Some(failure.code().into()),
                width: None,
                height: None,
            },
        )
        .await?;
        return Ok(());
    }

    let session_id: SessionId = match hello.session.parse() {
        Ok(id) => id,
        Err(_) => {
            write_ack(
                &mut writer,
                MediaHelloAck {
                    ok: false,
                    code: Some("SessionNotFound".into()),
                    width: None,
                    height: None,
                },
            )
            .await?;
            return Ok(());
        }
    };
    let session = match manager.info(&session_id) {
        Ok(session) => session,
        Err(_) => {
            write_ack(
                &mut writer,
                MediaHelloAck {
                    ok: false,
                    code: Some("SessionNotFound".into()),
                    width: None,
                    height: None,
                },
            )
            .await?;
            return Ok(());
        }
    };
    let Some(slot) = frames.get(&session_id) else {
        write_ack(
            &mut writer,
            MediaHelloAck {
                ok: false,
                code: Some("BackendUnavailable".into()),
                width: None,
                height: None,
            },
        )
        .await?;
        return Ok(());
    };

    write_ack(
        &mut writer,
        MediaHelloAck {
            ok: true,
            code: None,
            width: Some(session.resolution.width),
            height: Some(session.resolution.height),
        },
    )
    .await?;

    tokio::select! {
        result = receive_inputs(reader, slot.input.clone()) => result?,
        result = send_frames(&mut writer, slot) => result?,
    }
    Ok(())
}

pub(crate) async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::with_capacity(max_bytes.min(256));
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(IoError::new(ErrorKind::UnexpectedEof, "truncated line"))
            };
        }

        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if bytes.len().saturating_add(newline) > max_bytes {
                return Err(IoError::new(
                    ErrorKind::InvalidData,
                    "line exceeds maximum size",
                ));
            }
            bytes.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| IoError::new(ErrorKind::InvalidData, error));
        }

        if bytes.len().saturating_add(available.len()) > max_bytes {
            return Err(IoError::new(
                ErrorKind::InvalidData,
                "line exceeds maximum size",
            ));
        }
        bytes.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
}

async fn write_ack(writer: &mut OwnedWriteHalf, ack: MediaHelloAck) -> anyhow::Result<()> {
    let mut payload = serde_json::to_vec(&ack)?;
    payload.push(b'\n');
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn send_frames(
    writer: &mut OwnedWriteHalf,
    slot: Arc<SessionFrameSlot>,
) -> anyhow::Result<()> {
    loop {
        let frame = slot.take_latest().await;
        match tokio::time::timeout(Duration::from_secs(2), write_media_frame(writer, &frame)).await
        {
            Ok(result) => result?,
            Err(_) => anyhow::bail!("media client write timed out"),
        }
    }
}

async fn write_media_frame<W>(writer: &mut W, frame: &MediaFrame) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let header = encode_media_frame_header(frame)?;
    let total_len = header.len() + frame.pixels.len();
    let mut written = 0;

    while written < total_len {
        let count = if written < header.len() {
            let slices = [
                IoSlice::new(&header[written..]),
                IoSlice::new(&frame.pixels),
            ];
            writer.write_vectored(&slices).await?
        } else {
            writer
                .write(&frame.pixels[written - header.len()..])
                .await?
        };
        if count == 0 {
            return Err(IoError::new(ErrorKind::WriteZero, "failed to write media frame").into());
        }
        written += count;
    }
    Ok(())
}

async fn receive_inputs(
    mut reader: BufReader<OwnedReadHalf>,
    input: Option<SessionInputEndpoint>,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    let mut backend = InputBackendGuard(splitdesk_host_windows::WindowsInputBackend::new());
    #[cfg(windows)]
    let _ = &input;
    #[cfg(target_os = "linux")]
    let mut backend = {
        let input = input.ok_or_else(|| anyhow::anyhow!("Linux session input endpoint missing"))?;
        splitdesk_host_linux::WestonInputBackend::connect(&input.socket, input.uid)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
    };
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = input;

    while let Some(line) = read_bounded_line(&mut reader, MAX_JSON_LINE_BYTES).await? {
        if line.trim().is_empty() {
            continue;
        }
        let input: Input = serde_json::from_str(&line)?;
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = &input;
        #[cfg(windows)]
        if let Err(err) = backend.0.inject(&input) {
            tracing::warn!(error = %err, "Windows input injection failed");
        }
        #[cfg(target_os = "linux")]
        if let Err(err) = backend.inject(&input) {
            return Err(anyhow::anyhow!("Weston input injection failed: {err}"));
        }
    }
    Ok(())
}

#[cfg(windows)]
struct InputBackendGuard(splitdesk_host_windows::WindowsInputBackend);

#[cfg(windows)]
impl Drop for InputBackendGuard {
    fn drop(&mut self) {
        if let Err(err) = self.0.release_all() {
            tracing::warn!(error = %err, "failed to release Windows input state");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitdesk_protocol::encode_media_frame;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn vectored_frame_write_preserves_wire_format() {
        let frame = MediaFrame::bgra(2, 1, 42, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let expected = encode_media_frame(&frame);
        let (mut writer, mut reader) = tokio::io::duplex(expected.len());

        write_media_frame(&mut writer, &frame).await.unwrap();
        writer.shutdown().await.unwrap();

        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn bounded_line_reader_handles_lf_crlf_and_eof() {
        let input = b"first\nsecond\r\n";
        let mut reader = BufReader::new(&input[..]);

        assert_eq!(
            read_bounded_line(&mut reader, 16).await.unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(
            read_bounded_line(&mut reader, 16).await.unwrap().as_deref(),
            Some("second")
        );
        assert_eq!(read_bounded_line(&mut reader, 16).await.unwrap(), None);
    }

    #[tokio::test]
    async fn bounded_line_reader_rejects_oversize_invalid_and_truncated_lines() {
        for input in [&b"12345\n"[..], &b"\xff\n"[..], &b"abc"[..]] {
            let mut reader = BufReader::new(input);
            assert!(read_bounded_line(&mut reader, 4).await.is_err());
        }
    }
}
