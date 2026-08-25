use serde::de::DeserializeOwned;
use serde::Serialize;
use splitdesk_core::Error;
use std::io::Write;

pub const MAX_JSON_FRAME: u32 = 16 * 1024 * 1024;

pub fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let payload = serde_json::to_vec(value).map_err(|_| Error::Protocol)?;
    if payload.len() > MAX_JSON_FRAME as usize {
        return Err(Error::Protocol);
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

pub fn write_json<W: Write, T: Serialize>(mut writer: W, value: &T) -> Result<(), Error> {
    let bytes = encode_json(value)?;
    writer.write_all(&bytes).map_err(|_| Error::Io)?;
    Ok(())
}

pub fn try_decode_json<T: DeserializeOwned>(buf: &[u8]) -> Result<Option<(T, usize)>, Error> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if len > MAX_JSON_FRAME {
        return Err(Error::Protocol);
    }
    let total = 4 + len as usize;
    if buf.len() < total {
        return Ok(None);
    }
    let value = serde_json::from_slice(&buf[4..total]).map_err(|_| Error::Protocol)?;
    Ok(Some((value, total)))
}

#[derive(Default)]
pub struct JsonFrameDecoder {
    buf: Vec<u8>,
}

impl JsonFrameDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, Error> {
        match try_decode_json::<T>(&self.buf)? {
            None => {
                if self.buf.len() > MAX_JSON_FRAME as usize + 4 {
                    return Err(Error::Protocol);
                }
                Ok(None)
            }
            Some((value, consumed)) => {
                self.buf.drain(..consumed);
                Ok(Some(value))
            }
        }
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}
