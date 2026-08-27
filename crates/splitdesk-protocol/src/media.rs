use bytes::{Buf, Bytes, BytesMut};
use splitdesk_core::Error;

pub const MEDIA_MAGIC: [u8; 4] = *b"SDFR";
pub const MEDIA_FORMAT_BGRA: u32 = 1;
pub const MEDIA_HEADER_LEN: usize = 36;
pub const DEFAULT_MEDIA_BIND: &str = "127.0.0.1:9824";
pub const MAX_MEDIA_PIXELS: usize = 7680 * 4320 * 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub timestamp_ns: u64,
    pub cpu_copies: u32,
    /// Reference-counted storage keeps ownership transfers and decoder splits
    /// allocation-free. Constructing this from a `Vec<u8>` reuses its allocation.
    pub pixels: Bytes,
}

impl MediaFrame {
    pub fn bgra<B>(width: u32, height: u32, timestamp_ns: u64, cpu_copies: u32, pixels: B) -> Self
    where
        B: Into<Bytes>,
    {
        let stride = width.saturating_mul(4);
        Self {
            width,
            height,
            stride,
            timestamp_ns,
            cpu_copies,
            pixels: pixels.into(),
        }
    }
}

fn media_header(frame: &MediaFrame, payload_len: u32) -> [u8; MEDIA_HEADER_LEN] {
    let mut out = [0; MEDIA_HEADER_LEN];
    out[0..4].copy_from_slice(&MEDIA_MAGIC);
    out[4..8].copy_from_slice(&frame.width.to_le_bytes());
    out[8..12].copy_from_slice(&frame.height.to_le_bytes());
    out[12..16].copy_from_slice(&frame.stride.to_le_bytes());
    out[16..20].copy_from_slice(&MEDIA_FORMAT_BGRA.to_le_bytes());
    out[20..28].copy_from_slice(&frame.timestamp_ns.to_le_bytes());
    out[28..32].copy_from_slice(&frame.cpu_copies.to_le_bytes());
    out[32..36].copy_from_slice(&payload_len.to_le_bytes());
    out
}

/// Builds only the fixed-size wire header so callers can write the header and
/// shared pixel storage as separate buffers without copying the full frame.
pub fn encode_media_frame_header(frame: &MediaFrame) -> Result<[u8; MEDIA_HEADER_LEN], Error> {
    if frame.pixels.len() > MAX_MEDIA_PIXELS {
        return Err(Error::Protocol);
    }
    let payload_len = u32::try_from(frame.pixels.len()).map_err(|_| Error::Protocol)?;
    Ok(media_header(frame, payload_len))
}

/// Encodes a contiguous frame for compatibility and tests. Live transport uses
/// [`encode_media_frame_header`] plus vectored I/O to avoid this payload copy.
pub fn encode_media_frame(frame: &MediaFrame) -> Vec<u8> {
    let len = frame.pixels.len() as u32;
    let mut out = Vec::with_capacity(MEDIA_HEADER_LEN + frame.pixels.len());
    out.extend_from_slice(&media_header(frame, len));
    out.extend_from_slice(&frame.pixels);
    out
}

struct DecodedHeader {
    width: u32,
    height: u32,
    stride: u32,
    timestamp_ns: u64,
    cpu_copies: u32,
    payload_len: usize,
}

fn try_decode_header(buf: &[u8]) -> Result<Option<DecodedHeader>, Error> {
    if buf.len() < MEDIA_HEADER_LEN {
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
    Ok(Some(DecodedHeader {
        width,
        height,
        stride,
        timestamp_ns,
        cpu_copies,
        payload_len,
    }))
}

pub fn try_decode_media_frame(buf: &mut Vec<u8>) -> Result<Option<MediaFrame>, Error> {
    let Some(header) = try_decode_header(buf)? else {
        return Ok(None);
    };
    if buf.len() < MEDIA_HEADER_LEN + header.payload_len {
        return Ok(None);
    }
    let pixels =
        Bytes::copy_from_slice(&buf[MEDIA_HEADER_LEN..MEDIA_HEADER_LEN + header.payload_len]);
    buf.drain(..MEDIA_HEADER_LEN + header.payload_len);
    Ok(Some(MediaFrame {
        width: header.width,
        height: header.height,
        stride: header.stride,
        timestamp_ns: header.timestamp_ns,
        cpu_copies: header.cpu_copies,
        pixels,
    }))
}

/// Decodes one frame by splitting the payload from `buf` without allocating or
/// copying it. Any following frame remains buffered for the next call.
pub fn try_decode_media_frame_bytes(buf: &mut BytesMut) -> Result<Option<MediaFrame>, Error> {
    let Some(header) = try_decode_header(buf)? else {
        return Ok(None);
    };
    if buf.len() < MEDIA_HEADER_LEN + header.payload_len {
        return Ok(None);
    }
    buf.advance(MEDIA_HEADER_LEN);
    let pixels = buf.split_to(header.payload_len).freeze();
    Ok(Some(MediaFrame {
        width: header.width,
        height: header.height,
        stride: header.stride,
        timestamp_ns: header.timestamp_ns,
        cpu_copies: header.cpu_copies,
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

    #[test]
    fn bgra_reuses_owned_pixel_allocation() {
        let pixels = vec![1, 2, 3, 4];
        let allocation = pixels.as_ptr();
        let frame = MediaFrame::bgra(1, 1, 1, 1, pixels);
        assert_eq!(frame.pixels.as_ptr(), allocation);
    }

    #[test]
    fn bytes_decoder_splits_payload_without_copying() {
        let first = MediaFrame::bgra(1, 1, 1, 1, vec![1, 2, 3, 4]);
        let second = MediaFrame::bgra(1, 1, 2, 1, vec![5, 6, 7, 8]);
        let first_wire = encode_media_frame(&first);
        let mut wire = BytesMut::from(first_wire.as_slice());
        wire.extend_from_slice(&encode_media_frame(&second));
        let first_payload = wire[MEDIA_HEADER_LEN..].as_ptr();

        let decoded_first = try_decode_media_frame_bytes(&mut wire).unwrap().unwrap();
        assert_eq!(decoded_first, first);
        assert_eq!(decoded_first.pixels.as_ptr(), first_payload);
        assert_eq!(
            try_decode_media_frame_bytes(&mut wire).unwrap(),
            Some(second)
        );
        assert!(wire.is_empty());
    }

    #[test]
    fn standalone_header_matches_contiguous_encoding() {
        let frame = MediaFrame::bgra(1, 1, 7, 2, vec![1, 2, 3, 4]);
        let header = encode_media_frame_header(&frame).unwrap();
        let encoded = encode_media_frame(&frame);
        assert_eq!(header.as_slice(), &encoded[..MEDIA_HEADER_LEN]);
    }
}
