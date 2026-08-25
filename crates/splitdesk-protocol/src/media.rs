use splitdesk_core::Error;

pub const MEDIA_MAGIC: [u8; 4] = *b"SDFR";
pub const MEDIA_FORMAT_BGRA: u32 = 1;
pub const DEFAULT_MEDIA_BIND: &str = "127.0.0.1:9824";
pub const MAX_MEDIA_PIXELS: usize = 7680 * 4320 * 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub timestamp_ns: u64,
    pub cpu_copies: u32,
    pub pixels: Vec<u8>,
}

impl MediaFrame {
    pub fn bgra(
        width: u32,
        height: u32,
        timestamp_ns: u64,
        cpu_copies: u32,
        pixels: Vec<u8>,
    ) -> Self {
        let stride = width.saturating_mul(4);
        Self {
            width,
            height,
            stride,
            timestamp_ns,
            cpu_copies,
            pixels,
        }
    }
}

pub fn encode_media_frame(frame: &MediaFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(36 + frame.pixels.len());
    out.extend_from_slice(&MEDIA_MAGIC);
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&frame.stride.to_le_bytes());
    out.extend_from_slice(&MEDIA_FORMAT_BGRA.to_le_bytes());
    out.extend_from_slice(&frame.timestamp_ns.to_le_bytes());
    out.extend_from_slice(&frame.cpu_copies.to_le_bytes());
    let len = frame.pixels.len() as u32;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&frame.pixels);
    out
}

pub fn try_decode_media_frame(buf: &mut Vec<u8>) -> Result<Option<MediaFrame>, Error> {
    if buf.len() < 36 {
        return Ok(None);
    }
    if buf[0..4] != MEDIA_MAGIC {
        return Err(Error::Protocol);
    }
    let width = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    let height = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    let stride = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    let format = u32::from_le_bytes(buf[16..20].try_into().unwrap());
    let timestamp_ns = u64::from_le_bytes(buf[20..28].try_into().unwrap());
    let cpu_copies = u32::from_le_bytes(buf[28..32].try_into().unwrap());
    let payload_len = u32::from_le_bytes(buf[32..36].try_into().unwrap()) as usize;
    if format != MEDIA_FORMAT_BGRA {
        return Err(Error::Protocol);
    }
    if payload_len > MAX_MEDIA_PIXELS {
        return Err(Error::Protocol);
    }
    if buf.len() < 36 + payload_len {
        return Ok(None);
    }
    let pixels = buf[36..36 + payload_len].to_vec();
    buf.drain(..36 + payload_len);
    Ok(Some(MediaFrame {
        width,
        height,
        stride,
        timestamp_ns,
        cpu_copies,
        pixels,
    }))
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MediaHello {
    pub token: String,
    pub session: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MediaHelloAck {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_frame_roundtrip() {
        let frame = MediaFrame::bgra(2, 1, 42, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let bytes = encode_media_frame(&frame);
        let mut buf = bytes;
        let decoded = try_decode_media_frame(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, frame);
        assert!(buf.is_empty());
    }

    #[test]
    fn media_frame_waits_for_payload() {
        let frame = MediaFrame::bgra(1, 1, 1, 1, vec![9, 8, 7, 6]);
        let bytes = encode_media_frame(&frame);
        let mut partial = bytes[..20].to_vec();
        assert!(try_decode_media_frame(&mut partial).unwrap().is_none());
    }
}
