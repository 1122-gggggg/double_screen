use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use splitdesk_core::SessionId;
use splitdesk_media::BoundedSlot;
use splitdesk_protocol::{encode_media_frame, Input, MediaFrame, MediaHello, MediaHelloAck};
use splitdesk_session::SessionManager;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use crate::auth::authorize;
use crate::token::Token;

pub(crate) struct SessionFrameSlot {
    latest: BoundedSlot<MediaFrame>,
    ready: Notify,
    live_capture: AtomicBool,
}

impl SessionFrameSlot {
    fn new() -> Self {
        Self {
            latest: BoundedSlot::new(),
            ready: Notify::new(),
            live_capture: AtomicBool::new(false),
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

    pub(crate) fn register(&self, id: SessionId) -> Arc<SessionFrameSlot> {
        let slot = Arc::new(SessionFrameSlot::new());
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
    loop {
        match listener.accept().await {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                let token = Arc::clone(&token);
                let manager = Arc::clone(&manager);
                let frames = Arc::clone(&frames);
                tokio::spawn(async move {
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
    let mut lines = BufReader::new(reader).lines();

    let Some(line) = lines.next_line().await? else {
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
        result = receive_inputs(lines) => result?,
        result = send_frames(&mut writer, slot) => result?,
    }
    Ok(())
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
        let payload = encode_media_frame(&frame);
        match tokio::time::timeout(Duration::from_secs(2), writer.write_all(&payload)).await {
            Ok(result) => result?,
            Err(_) => anyhow::bail!("media client write timed out"),
        }
    }
}

async fn receive_inputs(mut lines: Lines<BufReader<OwnedReadHalf>>) -> anyhow::Result<()> {
    #[cfg(windows)]
    let mut backend = InputBackendGuard(splitdesk_host_windows::WindowsInputBackend::new());

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let input: Input = serde_json::from_str(&line)?;
        #[cfg(windows)]
        if let Err(err) = backend.0.inject(&input) {
            tracing::warn!(error = %err, "Windows input injection failed");
        }
        #[cfg(not(windows))]
        {
            let _ = input;
            tracing::warn!("remote input ignored on non-Windows host");
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
