//! OS-agnostic SplitDesk client: attach/detach, local cursor, latest-frame slot.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::BytesMut;
use minifb::{Key, KeyRepeat, MouseButton, MouseMode, Scale, Window, WindowOptions};
use openh264::{decoder::Decoder as H264Decoder, formats::YUVSource};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use splitdesk_core::{Codec, Error, SessionId, UserName};
use splitdesk_media::BoundedSlot;
use splitdesk_protocol::{
    try_decode_media_frame_bytes, Hello, Input, InputCaps, InputChannels, MediaFormat, MediaFrame,
    MediaHello, MediaHelloAck, Position, DEFAULT_MEDIA_BIND, PROTOCOL_VERSION,
};
use splitdeskd::{rpc_with_token, DaemonCommand, DaemonResult, DEFAULT_BIND};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

pub const OVERLAY_CHORD: &str = "Ctrl+Shift+F12";
pub const MOD_CTRL: u32 = 1 << 0;
pub const MOD_SHIFT: u32 = 1 << 1;
pub const KEY_F12_WIN: u32 = 123;
pub const KEY_F12_EVDEV: u32 = 88;
const MEDIA_WRITER_CAPACITY: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ClientStats {
    pub connected: bool,
    pub overlay: bool,
    pub fullscreen: bool,
    pub frames_dropped: u64,
    pub session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LocalCursor {
    local: Position,
    server: Position,
}

impl Default for LocalCursor {
    fn default() -> Self {
        Self {
            local: Position { x: 0.0, y: 0.0 },
            server: Position { x: 0.0, y: 0.0 },
        }
    }
}

impl LocalCursor {
    /// Immediate local move; the pointer is shown here without waiting on RTT.
    pub fn move_local(&mut self, x: f64, y: f64) {
        self.local = Position { x, y };
    }

    /// Server-reported position. Snap only when the gap is large.
    pub fn reconcile(&mut self, server: Position) {
        self.server = server;
        let dx = self.local.x - server.x;
        let dy = self.local.y - server.y;
        if dx * dx + dy * dy > 64.0 * 64.0 {
            self.local = server;
        }
    }

    pub fn displayed(&self) -> Position {
        self.local
    }

    pub fn server(&self) -> Position {
        self.server
    }
}

pub fn is_overlay_chord(keycode: u32, pressed: bool, modifiers: u32) -> bool {
    if !pressed {
        return false;
    }
    let ctrl_shift = (modifiers & (MOD_CTRL | MOD_SHIFT)) == (MOD_CTRL | MOD_SHIFT);
    ctrl_shift && (keycode == KEY_F12_WIN || keycode == KEY_F12_EVDEV || keycode == 0xffc9)
}

pub fn client_hello(client_name: impl Into<String>) -> Hello {
    Hello {
        protocol_versions: vec![PROTOCOL_VERSION],
        codecs: vec![Codec::H264],
        input_caps: InputCaps::default(),
        client_name: client_name.into(),
    }
}

enum MediaWriterCommand {
    Input(Input),
    Flush(oneshot::Sender<()>),
    Close,
}

pub struct ClientCore {
    server: String,
    token: String,
    user: String,
    media_server: Option<SocketAddr>,
    session_id: Option<SessionId>,
    overlay: bool,
    cursor: LocalCursor,
    frames: Arc<BoundedSlot<MediaFrame>>,
    input: Mutex<InputChannels>,
    media_writer: Option<mpsc::Sender<MediaWriterCommand>>,
    media_reader_task: Option<JoinHandle<()>>,
    media_writer_task: Option<JoinHandle<()>>,
    connected: bool,
}

impl ClientCore {
    pub fn new(
        server: impl Into<String>,
        user: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            server: server.into(),
            token: token.into(),
            user: user.into(),
            media_server: None,
            session_id: None,
            overlay: false,
            cursor: LocalCursor::default(),
            frames: Arc::new(BoundedSlot::new()),
            input: Mutex::new(InputChannels::new()),
            media_writer: None,
            media_reader_task: None,
            media_writer_task: None,
            connected: false,
        }
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    pub fn overlay_visible(&self) -> bool {
        self.overlay
    }

    pub fn cursor(&self) -> &LocalCursor {
        &self.cursor
    }

    pub fn cursor_mut(&mut self) -> &mut LocalCursor {
        &mut self.cursor
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    /// Override the daemon-advertised media endpoint, primarily for an SSH port forward.
    /// Both configured and advertised endpoints remain loopback-only until an encrypted
    /// network transport is implemented.
    pub fn set_media_server(&mut self, endpoint: Option<&str>) -> Result<(), Error> {
        self.media_server = endpoint.map(parse_loopback_media_endpoint).transpose()?;
        Ok(())
    }

    pub fn push_frame(&self, frame: MediaFrame) {
        self.frames.push(frame);
    }

    pub fn take_frame(&self) -> Option<MediaFrame> {
        self.frames.take()
    }

    pub fn frames_dropped(&self) -> u64 {
        self.frames.dropped()
    }

    pub fn handle_key(
        &mut self,
        keycode: u32,
        pressed: bool,
        modifiers: u32,
        ts: u64,
    ) -> Result<(), Error> {
        if is_overlay_chord(keycode, pressed, modifiers) {
            self.overlay = !self.overlay;
            tracing::info!(
                overlay = self.overlay,
                chord = OVERLAY_CHORD,
                "overlay toggled"
            );
            return Ok(());
        }
        self.send_input(Input::Key {
            keycode,
            pressed,
            modifiers,
            ts,
        })
    }

    pub fn send_input(&self, input: Input) -> Result<(), Error> {
        self.input.lock().push(input)
    }

    /// Move queued input into the connected writer and return only events accepted by it.
    /// Without a connected writer, returns the locally drained events for embedders/tests.
    pub fn drain_outbound(&self) -> Vec<Input> {
        let mut channels = self.input.lock();
        let mut out = Vec::new();
        if let Some(motion) = channels.take_motion() {
            out.push(motion);
        }
        if let Some(scroll) = channels.take_scroll() {
            out.push(scroll);
        }
        while let Some(reliable) = channels.pop_reliable() {
            out.push(reliable);
        }
        let mut accepted = out.len();
        if let Some(writer) = &self.media_writer {
            for (index, input) in out.iter().cloned().enumerate() {
                if let Err(error) = writer.try_send(MediaWriterCommand::Input(input)) {
                    accepted = index;
                    for unsent in out[index..].iter().cloned() {
                        if channels.push(unsent).is_err() {
                            tracing::warn!("input retry queue reached its bounded capacity");
                            break;
                        }
                    }
                    match error {
                        mpsc::error::TrySendError::Full(_) => {
                            tracing::debug!("media input writer is applying backpressure");
                        }
                        mpsc::error::TrySendError::Closed(_) => {
                            tracing::warn!("media input writer is closed");
                        }
                    }
                    break;
                }
            }
        }
        drop(channels);
        out.truncate(accepted);
        out
    }

    pub async fn attach(&mut self, session_id: SessionId) -> Result<SessionId, Error> {
        self.stop_media().await;
        let result = rpc_with_token(
            &self.server,
            &self.token,
            DaemonCommand::Attach {
                id: session_id.to_string(),
            },
        )
        .await
        .map_err(|_| Error::Auth)?;
        let (id, media_bind) = session_from_result(result)?;
        self.session_id = Some(id);
        if let Err(error) = self.connect_media(id, media_bind.as_deref()).await {
            let _ = rpc_with_token(
                &self.server,
                &self.token,
                DaemonCommand::Disconnect { id: id.to_string() },
            )
            .await;
            self.connected = false;
            return Err(error);
        }
        self.connected = true;
        Ok(id)
    }

    /// Create a session for `user` then attach. Reconnect should call [`Self::attach`] instead.
    pub async fn connect_new(&mut self) -> Result<SessionId, Error> {
        let created = rpc_with_token(
            &self.server,
            &self.token,
            DaemonCommand::SessionCreate {
                user: self.user.clone(),
                width: None,
                height: None,
                fps: None,
                memory_limit: None,
                cpu_affinity: None,
            },
        )
        .await
        .map_err(|_| Error::Auth)?;
        let (id, _) = session_from_result(created)?;
        self.attach(id).await
    }

    /// Reconnect to the existing session; does not destroy it.
    pub async fn reconnect(&mut self) -> Result<SessionId, Error> {
        let id = self.session_id.ok_or(Error::SessionNotFound)?;
        self.attach(id).await
    }

    /// Detach. The host session remains.
    pub async fn detach(&mut self) -> Result<(), Error> {
        let id = self.session_id.ok_or(Error::SessionNotFound)?;
        let result = rpc_with_token(
            &self.server,
            &self.token,
            DaemonCommand::Detach { id: id.to_string() },
        )
        .await
        .map_err(|_| Error::Auth);
        self.stop_media().await;
        self.connected = false;
        result.map(|_| ())
    }

    /// Drop the client connection. Releases held input, then disconnects before closing media.
    pub async fn disconnect(&mut self) -> Result<(), Error> {
        let released = self.input.lock().release_held(0);
        if let Some(writer) = &self.media_writer {
            for input in released {
                let sent = tokio::time::timeout(
                    Duration::from_millis(500),
                    writer.send(MediaWriterCommand::Input(input)),
                )
                .await;
                if !matches!(sent, Ok(Ok(()))) {
                    tracing::warn!("timed out while releasing held input");
                    break;
                }
            }
        }
        self.flush_media_writer().await;

        let result = if let Some(id) = self.session_id {
            rpc_with_token(
                &self.server,
                &self.token,
                DaemonCommand::Disconnect { id: id.to_string() },
            )
            .await
            .map(|_| ())
            .map_err(|_| Error::Auth)
        } else {
            Ok(())
        };
        self.stop_media().await;
        self.connected = false;
        result
    }

    async fn connect_media(
        &mut self,
        session_id: SessionId,
        media_bind: Option<&str>,
    ) -> Result<(), Error> {
        let address = match self.media_server {
            Some(address) => address,
            None => parse_loopback_media_endpoint(media_bind.unwrap_or(DEFAULT_MEDIA_BIND))?,
        };

        let mut stream = TcpStream::connect(address).await.map_err(|_| Error::Io)?;
        let hello = MediaHello {
            token: self.token.clone(),
            session: session_id.to_string(),
        };
        let mut hello_line = serde_json::to_vec(&hello).map_err(|_| Error::Protocol)?;
        hello_line.push(b'\n');
        stream.write_all(&hello_line).await.map_err(|_| Error::Io)?;

        let mut buffered = BufReader::new(stream);
        let mut ack_line = String::new();
        let ack_len = buffered
            .read_line(&mut ack_line)
            .await
            .map_err(|_| Error::Io)?;
        if ack_len == 0 || ack_len > 8 * 1024 {
            return Err(Error::Protocol);
        }
        let ack: MediaHelloAck =
            serde_json::from_str(ack_line.trim_end()).map_err(|_| Error::Protocol)?;
        if !ack.ok {
            return Err(if ack.code.as_deref() == Some("Auth") {
                Error::Auth
            } else {
                Error::Protocol
            });
        }

        let pending = BytesMut::from(buffered.buffer());
        let stream = buffered.into_inner();
        let (mut reader, mut writer) = stream.into_split();
        let frames = Arc::clone(&self.frames);
        self.media_reader_task = Some(tokio::spawn(async move {
            let mut bytes = pending;
            let mut h264_decoder: Option<H264Decoder> = None;
            loop {
                loop {
                    match try_decode_media_frame_bytes(&mut bytes) {
                        Ok(Some(frame)) => match decode_media_frame(frame, &mut h264_decoder) {
                            Ok(Some(frame)) => {
                                frames.push(frame);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                tracing::warn!(%error, "H.264 media decode failed");
                            }
                        },
                        Ok(None) => break,
                        Err(_) => {
                            tracing::warn!("media stream protocol error");
                            return;
                        }
                    }
                }
                match reader.read_buf(&mut bytes).await {
                    Ok(0) => return,
                    Ok(_) => {}
                    Err(_) => {
                        tracing::warn!("media stream read failed");
                        return;
                    }
                }
            }
        }));

        let (writer_tx, mut writer_rx) = mpsc::channel(MEDIA_WRITER_CAPACITY);
        self.media_writer_task = Some(tokio::spawn(async move {
            while let Some(command) = writer_rx.recv().await {
                match command {
                    MediaWriterCommand::Input(input) => {
                        let mut line = match serde_json::to_vec(&input) {
                            Ok(line) => line,
                            Err(_) => {
                                tracing::warn!("input serialization failed");
                                continue;
                            }
                        };
                        line.push(b'\n');
                        if writer.write_all(&line).await.is_err() {
                            tracing::warn!("media input write failed");
                            return;
                        }
                    }
                    MediaWriterCommand::Flush(done) => {
                        let _ = writer.flush().await;
                        let _ = done.send(());
                    }
                    MediaWriterCommand::Close => {
                        let _ = writer.shutdown().await;
                        return;
                    }
                }
            }
            let _ = writer.shutdown().await;
        }));
        self.media_writer = Some(writer_tx);
        Ok(())
    }

    async fn flush_media_writer(&self) {
        let Some(writer) = &self.media_writer else {
            return;
        };
        let (done_tx, done_rx) = oneshot::channel();
        let sent = tokio::time::timeout(
            Duration::from_millis(500),
            writer.send(MediaWriterCommand::Flush(done_tx)),
        )
        .await;
        if matches!(sent, Ok(Ok(()))) {
            let _ = tokio::time::timeout(Duration::from_millis(500), done_rx).await;
        }
    }

    async fn stop_media(&mut self) {
        if let Some(writer) = self.media_writer.take() {
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                writer.send(MediaWriterCommand::Close),
            )
            .await;
        }
        if let Some(mut task) = self.media_writer_task.take() {
            if tokio::time::timeout(Duration::from_secs(1), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        if let Some(task) = self.media_reader_task.take() {
            task.abort();
            let _ = task.await;
        }
    }

    pub fn stats(&self, fullscreen: bool) -> ClientStats {
        ClientStats {
            connected: self.connected,
            overlay: self.overlay,
            fullscreen,
            frames_dropped: self.frames.dropped(),
            session_id: self.session_id.map(|id| id.to_string()),
        }
    }
}

fn parse_loopback_media_endpoint(endpoint: &str) -> Result<SocketAddr, Error> {
    let address = endpoint
        .parse::<SocketAddr>()
        .map_err(|_| Error::Protocol)?;
    if !address.ip().is_loopback() {
        return Err(Error::Protocol);
    }
    Ok(address)
}

fn session_from_result(result: DaemonResult) -> Result<(SessionId, Option<String>), Error> {
    match result {
        DaemonResult::Session {
            session,
            media_bind,
        } => Ok((session.id, media_bind)),
        _ => Err(Error::Protocol),
    }
}

/// Native minifb window client. No Electron.
pub struct WindowedClient {
    pub server: String,
    pub user: String,
    pub fullscreen: bool,
    pub connected: bool,
    core: ClientCore,
}

impl WindowedClient {
    pub fn new(
        server: impl Into<String>,
        user: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        let server = server.into();
        let user = user.into();
        let core = ClientCore::new(server.clone(), user.clone(), token);
        Self {
            server,
            user,
            fullscreen: false,
            connected: false,
            core,
        }
    }

    pub fn default_server() -> &'static str {
        DEFAULT_BIND
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        self.fullscreen = on;
    }

    pub fn set_media_server(&mut self, endpoint: Option<&str>) -> Result<(), Error> {
        self.core.set_media_server(endpoint)
    }

    pub fn core(&self) -> &ClientCore {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut ClientCore {
        &mut self.core
    }

    pub fn handle_cursor_move(&mut self, x: f64, y: f64, ts: u64) {
        self.core.cursor_mut().move_local(x, y);
        let _ = self.core.send_input(Input::PointerMotion { x, y, ts });
    }

    pub fn handle_server_cursor(&mut self, position: Position) {
        self.core.cursor_mut().reconcile(position);
    }

    pub fn handle_key(&mut self, keycode: u32, pressed: bool, ctrl: bool, shift: bool, ts: u64) {
        let mut modifiers = 0;
        if ctrl {
            modifiers |= MOD_CTRL;
        }
        if shift {
            modifiers |= MOD_SHIFT;
        }
        let _ = self.core.handle_key(keycode, pressed, modifiers, ts);
    }

    pub async fn connect(&mut self, session_id: Option<SessionId>) -> Result<SessionId, Error> {
        let id = match session_id {
            Some(id) => self.core.attach(id).await?,
            None => self.core.connect_new().await?,
        };
        self.connected = true;
        Ok(id)
    }

    pub async fn disconnect(&mut self) -> Result<(), Error> {
        let result = self.core.disconnect().await;
        self.connected = false;
        result
    }

    pub fn run_loop(&mut self) -> Result<(), Error> {
        if !self.connected {
            return Err(Error::SessionNotFound);
        }

        let mut frame_width = 1280usize;
        let mut frame_height = 720usize;
        let mut display = vec![0; frame_width * frame_height];
        let mut capture_copies = 0u32;
        let mut presented = 0u64;
        if let Some(frame) = self.core.take_frame() {
            if let Some((width, height)) = bgra_to_minifb(&frame, &mut display) {
                frame_width = width;
                frame_height = height;
                capture_copies = frame.cpu_copies;
                presented = 1;
            }
        }

        let options = WindowOptions {
            borderless: self.fullscreen,
            scale: if self.fullscreen {
                Scale::FitScreen
            } else {
                Scale::X1
            },
            ..WindowOptions::default()
        };
        let mut window =
            Window::new("SplitDesk", frame_width, frame_height, options).map_err(|_| Error::Io)?;
        window.set_target_fps(60);

        let started = Instant::now();
        let mut fps_started = Instant::now();
        let mut fps = 0.0f64;
        let mut last_mouse = None;
        let mut buttons_down = [false; 3];
        let mut suppress_f12_release = false;
        let mut overlay_was_visible = false;
        let mut title_updated = Instant::now();

        while window.is_open() {
            let timestamp_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let (window_width, window_height) = window.get_size();

            if let Some(mouse) = window.get_mouse_pos(MouseMode::Clamp) {
                if last_mouse != Some(mouse) {
                    let x = mouse.0 as f64 * frame_width as f64 / window_width.max(1) as f64;
                    let y = mouse.1 as f64 * frame_height as f64 / window_height.max(1) as f64;
                    self.handle_cursor_move(x, y, timestamp_ns);
                    last_mouse = Some(mouse);
                }
            }

            for (index, button) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
                .into_iter()
                .enumerate()
            {
                let down = window.get_mouse_down(button);
                if down != buttons_down[index] {
                    self.core.send_input(Input::PointerButton {
                        button: index as u32 + 1,
                        pressed: down,
                        ts: timestamp_ns,
                    })?;
                    buttons_down[index] = down;
                }
            }

            if let Some((dx, dy)) = window.get_scroll_wheel() {
                if dx != 0.0 || dy != 0.0 {
                    self.core.send_input(Input::Scroll {
                        dx: dx as f64,
                        dy: dy as f64,
                        ts: timestamp_ns,
                    })?;
                }
            }

            let ctrl = window.is_key_down(Key::LeftCtrl) || window.is_key_down(Key::RightCtrl);
            let shift = window.is_key_down(Key::LeftShift) || window.is_key_down(Key::RightShift);
            let mut modifiers = 0;
            if ctrl {
                modifiers |= MOD_CTRL;
            }
            if shift {
                modifiers |= MOD_SHIFT;
            }
            for key in window.get_keys_pressed(KeyRepeat::No) {
                if key == Key::F12 && ctrl && shift {
                    self.core
                        .handle_key(KEY_F12_WIN, true, modifiers, timestamp_ns)?;
                    suppress_f12_release = true;
                } else if let Some(keycode) = minifb_key_to_vk(key) {
                    self.core
                        .handle_key(keycode, true, modifiers, timestamp_ns)?;
                }
            }
            for key in window.get_keys_released() {
                if key == Key::F12 && suppress_f12_release {
                    suppress_f12_release = false;
                } else if let Some(keycode) = minifb_key_to_vk(key) {
                    self.core
                        .handle_key(keycode, false, modifiers, timestamp_ns)?;
                }
            }
            let _ = self.core.drain_outbound();

            if let Some(frame) = self.core.take_frame() {
                if let Some((width, height)) = bgra_to_minifb(&frame, &mut display) {
                    if width != frame_width || height != frame_height {
                        last_mouse = None;
                    }
                    frame_width = width;
                    frame_height = height;
                    capture_copies = frame.cpu_copies;
                    presented = presented.saturating_add(1);
                }
            }

            let fps_elapsed = fps_started.elapsed();
            if fps_elapsed >= Duration::from_secs(1) {
                fps = presented as f64 / fps_elapsed.as_secs_f64();
                presented = 0;
                fps_started = Instant::now();
            }

            let overlay_visible = self.core.overlay_visible();
            if overlay_visible
                && (!overlay_was_visible || title_updated.elapsed() >= Duration::from_millis(250))
            {
                window.set_title(&format!(
                    "SplitDesk | fps={fps:.1} drops={} copies={} capture_copies={capture_copies}",
                    self.core.frames_dropped(),
                    capture_copies.saturating_add(1)
                ));
                title_updated = Instant::now();
            } else if !overlay_visible && overlay_was_visible {
                window.set_title("SplitDesk");
            }
            overlay_was_visible = overlay_visible;

            window
                .update_with_buffer(&display, frame_width, frame_height)
                .map_err(|_| Error::Io)?;
        }
        Ok(())
    }

    pub fn stats(&self) -> ClientStats {
        self.core.stats(self.fullscreen)
    }

    pub fn user_name(&self) -> UserName {
        UserName::new(self.user.clone())
    }
}

fn bgra_to_minifb(frame: &MediaFrame, out: &mut Vec<u32>) -> Option<(usize, usize)> {
    if frame.format != MediaFormat::Bgra {
        return None;
    }
    let width = usize::try_from(frame.width).ok()?;
    let height = usize::try_from(frame.height).ok()?;
    let stride = usize::try_from(frame.stride).ok()?;
    let row_bytes = width.checked_mul(4)?;
    let source_len = stride.checked_mul(height)?;
    let pixel_count = width.checked_mul(height)?;
    if width == 0 || height == 0 || stride < row_bytes || frame.pixels.len() < source_len {
        return None;
    }

    out.resize(pixel_count, 0);
    for y in 0..height {
        let source_start = y * stride;
        let source = &frame.pixels[source_start..source_start + row_bytes];
        let destination = &mut out[y * width..(y + 1) * width];
        for (pixel, bgra) in destination.iter_mut().zip(source.chunks_exact(4)) {
            *pixel = (u32::from(bgra[2]) << 16) | (u32::from(bgra[1]) << 8) | u32::from(bgra[0]);
        }
    }
    Some((width, height))
}

fn decode_media_frame(
    frame: MediaFrame,
    decoder: &mut Option<H264Decoder>,
) -> Result<Option<MediaFrame>, String> {
    if frame.format == MediaFormat::Bgra {
        return Ok(Some(frame));
    }

    let decoder = match decoder {
        Some(decoder) => decoder,
        slot @ None => {
            let created = H264Decoder::new().map_err(|error| error.to_string())?;
            slot.insert(created)
        }
    };
    let Some(yuv) = decoder
        .decode(&frame.pixels)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let (decoded_width, decoded_height) = yuv.dimensions();
    let width = u32::try_from(decoded_width).map_err(|_| "invalid decoded width".to_string())?;
    let height = u32::try_from(decoded_height).map_err(|_| "invalid decoded height".to_string())?;
    let len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "decoded frame size overflow".to_string())?;
    let mut rgba = vec![0; len];
    yuv.write_rgba8(&mut rgba);
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Some(MediaFrame::bgra(
        width,
        height,
        frame.timestamp_ns,
        frame.cpu_copies,
        rgba,
    )))
}

fn minifb_key_to_vk(key: Key) -> Option<u32> {
    Some(match key {
        Key::Key0 => 0x30,
        Key::Key1 => 0x31,
        Key::Key2 => 0x32,
        Key::Key3 => 0x33,
        Key::Key4 => 0x34,
        Key::Key5 => 0x35,
        Key::Key6 => 0x36,
        Key::Key7 => 0x37,
        Key::Key8 => 0x38,
        Key::Key9 => 0x39,
        Key::A => 0x41,
        Key::B => 0x42,
        Key::C => 0x43,
        Key::D => 0x44,
        Key::E => 0x45,
        Key::F => 0x46,
        Key::G => 0x47,
        Key::H => 0x48,
        Key::I => 0x49,
        Key::J => 0x4a,
        Key::K => 0x4b,
        Key::L => 0x4c,
        Key::M => 0x4d,
        Key::N => 0x4e,
        Key::O => 0x4f,
        Key::P => 0x50,
        Key::Q => 0x51,
        Key::R => 0x52,
        Key::S => 0x53,
        Key::T => 0x54,
        Key::U => 0x55,
        Key::V => 0x56,
        Key::W => 0x57,
        Key::X => 0x58,
        Key::Y => 0x59,
        Key::Z => 0x5a,
        Key::F1 => 0x70,
        Key::F2 => 0x71,
        Key::F3 => 0x72,
        Key::F4 => 0x73,
        Key::F5 => 0x74,
        Key::F6 => 0x75,
        Key::F7 => 0x76,
        Key::F8 => 0x77,
        Key::F9 => 0x78,
        Key::F10 => 0x79,
        Key::F11 => 0x7a,
        Key::F12 => 0x7b,
        Key::F13 => 0x7c,
        Key::F14 => 0x7d,
        Key::F15 => 0x7e,
        Key::Down => 0x28,
        Key::Left => 0x25,
        Key::Right => 0x27,
        Key::Up => 0x26,
        Key::Apostrophe => 0xde,
        Key::Backquote => 0xc0,
        Key::Backslash => 0xdc,
        Key::Comma => 0xbc,
        Key::Equal => 0xbb,
        Key::LeftBracket => 0xdb,
        Key::Minus => 0xbd,
        Key::Period => 0xbe,
        Key::RightBracket => 0xdd,
        Key::Semicolon => 0xba,
        Key::Slash => 0xbf,
        Key::Backspace => 0x08,
        Key::Delete => 0x2e,
        Key::End => 0x23,
        Key::Enter | Key::NumPadEnter => 0x0d,
        Key::Escape => 0x1b,
        Key::Home => 0x24,
        Key::Insert => 0x2d,
        Key::Menu => 0x5d,
        Key::PageDown => 0x22,
        Key::PageUp => 0x21,
        Key::Pause => 0x13,
        Key::Space => 0x20,
        Key::Tab => 0x09,
        Key::NumLock => 0x90,
        Key::CapsLock => 0x14,
        Key::ScrollLock => 0x91,
        Key::LeftShift => 0xa0,
        Key::RightShift => 0xa1,
        Key::LeftCtrl => 0xa2,
        Key::RightCtrl => 0xa3,
        Key::NumPad0 => 0x60,
        Key::NumPad1 => 0x61,
        Key::NumPad2 => 0x62,
        Key::NumPad3 => 0x63,
        Key::NumPad4 => 0x64,
        Key::NumPad5 => 0x65,
        Key::NumPad6 => 0x66,
        Key::NumPad7 => 0x67,
        Key::NumPad8 => 0x68,
        Key::NumPad9 => 0x69,
        Key::NumPadDot => 0x6e,
        Key::NumPadSlash => 0x6f,
        Key::NumPadAsterisk => 0x6a,
        Key::NumPadMinus => 0x6d,
        Key::NumPadPlus => 0x6b,
        Key::LeftAlt => 0xa4,
        Key::RightAlt => 0xa5,
        Key::LeftSuper => 0x5b,
        Key::RightSuper => 0x5c,
        Key::Unknown | Key::Count => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_chord_toggles() {
        let mut core = ClientCore::new(DEFAULT_BIND, "alice", "token");
        assert!(!core.overlay_visible());
        core.handle_key(KEY_F12_WIN, true, MOD_CTRL | MOD_SHIFT, 1)
            .unwrap();
        assert!(core.overlay_visible());
        core.handle_key(KEY_F12_WIN, true, MOD_CTRL | MOD_SHIFT, 2)
            .unwrap();
        assert!(!core.overlay_visible());
    }

    #[test]
    fn local_cursor_moves_immediately() {
        let mut cursor = LocalCursor::default();
        cursor.move_local(10.0, 20.0);
        assert_eq!(cursor.displayed().x, 10.0);
        cursor.reconcile(Position { x: 11.0, y: 21.0 });
        assert_eq!(cursor.displayed().x, 10.0);
        cursor.reconcile(Position { x: 400.0, y: 400.0 });
        assert_eq!(cursor.displayed().x, 400.0);
    }

    #[test]
    fn latest_frame_depth_one() {
        let core = ClientCore::new(DEFAULT_BIND, "alice", "token");
        core.push_frame(MediaFrame::bgra(2, 2, 1, 1, vec![0; 16]));
        core.push_frame(MediaFrame::bgra(2, 2, 2, 1, vec![1; 16]));
        assert_eq!(core.frames_dropped(), 1);
        let frame = core.take_frame().unwrap();
        assert_eq!(frame.timestamp_ns, 2);
        assert!(core.take_frame().is_none());
    }

    #[test]
    fn bgra_pixels_are_packed_as_zero_rgb() {
        let frame = MediaFrame::bgra(2, 1, 1, 2, vec![0x11, 0x22, 0x33, 0xff, 1, 2, 3, 4]);
        let mut out = Vec::new();
        assert_eq!(bgra_to_minifb(&frame, &mut out), Some((2, 1)));
        assert_eq!(out, vec![0x0033_2211, 0x0003_0201]);
    }

    #[test]
    fn motion_is_latest_wins() {
        let core = ClientCore::new(DEFAULT_BIND, "alice", "token");
        core.send_input(Input::PointerMotion {
            x: 1.0,
            y: 1.0,
            ts: 1,
        })
        .unwrap();
        core.send_input(Input::PointerMotion {
            x: 2.0,
            y: 2.0,
            ts: 2,
        })
        .unwrap();
        let out = core.drain_outbound();
        assert_eq!(out.len(), 1);
        match out[0] {
            Input::PointerMotion { x, ts, .. } => {
                assert_eq!(x, 2.0);
                assert_eq!(ts, 2);
            }
            _ => panic!("expected motion"),
        }
    }

    #[test]
    fn writer_backpressure_keeps_latest_input_for_retry() {
        let mut core = ClientCore::new(DEFAULT_BIND, "alice", "token");
        let (writer, _reader) = mpsc::channel(MEDIA_WRITER_CAPACITY);
        for index in 0..MEDIA_WRITER_CAPACITY {
            writer
                .try_send(MediaWriterCommand::Input(Input::Key {
                    keycode: index as u32,
                    pressed: true,
                    modifiers: 0,
                    ts: index as u64,
                }))
                .unwrap();
        }
        core.media_writer = Some(writer);
        core.send_input(Input::PointerMotion {
            x: 42.0,
            y: 24.0,
            ts: 9,
        })
        .unwrap();

        let accepted = core.drain_outbound();
        assert!(accepted.is_empty());
        core.media_writer = None;
        let retried = core.drain_outbound();

        assert!(matches!(
            retried.as_slice(),
            [Input::PointerMotion {
                x: 42.0,
                y: 24.0,
                ts: 9
            }]
        ));
    }

    #[test]
    fn media_override_accepts_only_loopback_endpoints() {
        let mut core = ClientCore::new(DEFAULT_BIND, "alice", "token");
        assert!(core.set_media_server(Some("127.0.0.1:19824")).is_ok());
        assert!(core.set_media_server(Some("[::1]:19824")).is_ok());
        assert!(core.set_media_server(Some("0.0.0.0:19824")).is_err());
        assert!(core.set_media_server(Some("192.0.2.10:19824")).is_err());
        assert!(core.set_media_server(Some("not-an-address")).is_err());
    }

    #[test]
    fn patched_openh264_decodes_a_generated_access_unit() {
        use openh264::encoder::Encoder;
        use openh264::formats::YUVBuffer;

        let mut encoder = Encoder::new().unwrap();
        let yuv = YUVBuffer::new(16, 16);
        let access_unit = encoder.encode(&yuv).unwrap().to_vec();
        let mut decoder = None;

        let decoded = decode_media_frame(MediaFrame::h264(16, 16, 7, access_unit), &mut decoder)
            .unwrap()
            .expect("generated access unit should decode");

        assert_eq!(decoded.format, MediaFormat::Bgra);
        assert_eq!((decoded.width, decoded.height), (16, 16));
        assert_eq!(decoded.pixels.len(), 16 * 16 * 4);
    }
}
