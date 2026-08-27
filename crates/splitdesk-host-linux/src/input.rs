#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::path::Path;

use splitdesk_core::Error;
use splitdesk_media::InputBackend;

#[cfg(any(target_os = "linux", test))]
const INPUT_MAGIC: u32 = 0x4e49_4453;
#[cfg(any(target_os = "linux", test))]
const INPUT_VERSION: u16 = 1;
#[cfg(any(target_os = "linux", test))]
const PACKET_LEN: usize = 40;
#[cfg(any(target_os = "linux", test))]
const POINTER_MOTION: u16 = 1;
#[cfg(any(target_os = "linux", test))]
const POINTER_BUTTON: u16 = 2;
#[cfg(any(target_os = "linux", test))]
const KEY: u16 = 3;
#[cfg(any(target_os = "linux", test))]
const SCROLL: u16 = 4;

#[cfg(target_os = "linux")]
pub struct WestonInputBackend {
    stream: std::os::unix::net::UnixStream,
    keys_down: HashSet<u32>,
    buttons_down: HashSet<u32>,
}

#[cfg(not(target_os = "linux"))]
pub struct WestonInputBackend;

impl WestonInputBackend {
    #[cfg(target_os = "linux")]
    pub fn connect(path: &Path, expected_uid: u32) -> Result<Self, Error> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        let metadata = std::fs::metadata(path)
            .map_err(|error| Error::backend(format!("stat {}: {error}", path.display())))?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != expected_uid
            || metadata.mode() & 0o077 != 0
        {
            return Err(Error::IsolationViolation);
        }
        let stream = std::os::unix::net::UnixStream::connect(path).map_err(|error| {
            Error::backend(format!(
                "connect compositor input socket {}: {error}",
                path.display()
            ))
        })?;
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(1)))
            .map_err(|_| Error::Io)?;
        Ok(Self {
            stream,
            keys_down: HashSet::new(),
            buttons_down: HashSet::new(),
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn connect(_path: &Path, _expected_uid: u32) -> Result<Self, Error> {
        Err(Error::backend(
            "Weston input backend is only available on Linux",
        ))
    }

    pub fn inject(&mut self, input: &splitdesk_protocol::Input) -> Result<(), Error> {
        InputBackend::inject(self, input.clone().into())
    }
}

#[cfg(target_os = "linux")]
impl WestonInputBackend {
    fn send(&mut self, packet: [u8; PACKET_LEN]) -> Result<(), Error> {
        use std::io::Write;
        self.stream.write_all(&packet).map_err(|_| Error::Io)
    }
}

impl InputBackend for WestonInputBackend {
    fn motion(&mut self, x: f64, y: f64, ts: u64) -> Result<(), Error> {
        #[cfg(target_os = "linux")]
        return self.send(packet(POINTER_MOTION, ts, x.to_bits(), y.to_bits(), 0, 0));
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (x, y, ts);
            Err(Error::Unsupported)
        }
    }

    fn button(&mut self, button: u32, pressed: bool, ts: u64) -> Result<(), Error> {
        if !(1..=5).contains(&button) {
            return Err(Error::Protocol);
        }
        #[cfg(target_os = "linux")]
        {
            self.send(packet(POINTER_BUTTON, ts, 0, 0, button, u32::from(pressed)))?;
            if pressed {
                self.buttons_down.insert(button);
            } else {
                self.buttons_down.remove(&button);
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (button, pressed, ts);
            Err(Error::Unsupported)
        }
    }

    fn key(&mut self, keycode: u32, pressed: bool, _modifiers: u32, ts: u64) -> Result<(), Error> {
        if !supported_vk(keycode) {
            return Err(Error::Protocol);
        }
        #[cfg(target_os = "linux")]
        {
            self.send(packet(KEY, ts, 0, 0, keycode, u32::from(pressed)))?;
            if pressed {
                self.keys_down.insert(keycode);
            } else {
                self.keys_down.remove(&keycode);
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (keycode, pressed, ts);
            Err(Error::Unsupported)
        }
    }

    fn scroll(&mut self, dx: f64, dy: f64, ts: u64) -> Result<(), Error> {
        #[cfg(target_os = "linux")]
        return self.send(packet(SCROLL, ts, dx.to_bits(), dy.to_bits(), 0, 0));
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (dx, dy, ts);
            Err(Error::Unsupported)
        }
    }

    fn release_all(&mut self) -> Result<(), Error> {
        #[cfg(target_os = "linux")]
        {
            let keys: Vec<u32> = self.keys_down.drain().collect();
            let buttons: Vec<u32> = self.buttons_down.drain().collect();
            let mut result = Ok(());
            for key in keys {
                if self.send(packet(KEY, 0, 0, 0, key, 0)).is_err() {
                    result = Err(Error::Io);
                }
            }
            for button in buttons {
                if self
                    .send(packet(POINTER_BUTTON, 0, 0, 0, button, 0))
                    .is_err()
                {
                    result = Err(Error::Io);
                }
            }
            result
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for WestonInputBackend {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

#[cfg(any(target_os = "linux", test))]
fn packet(kind: u16, ts: u64, a: u64, b: u64, c: u32, d: u32) -> [u8; PACKET_LEN] {
    let mut bytes = [0; PACKET_LEN];
    bytes[0..4].copy_from_slice(&INPUT_MAGIC.to_le_bytes());
    bytes[4..6].copy_from_slice(&INPUT_VERSION.to_le_bytes());
    bytes[6..8].copy_from_slice(&kind.to_le_bytes());
    bytes[8..16].copy_from_slice(&ts.to_le_bytes());
    bytes[16..24].copy_from_slice(&a.to_le_bytes());
    bytes[24..32].copy_from_slice(&b.to_le_bytes());
    bytes[32..36].copy_from_slice(&c.to_le_bytes());
    bytes[36..40].copy_from_slice(&d.to_le_bytes());
    bytes
}

fn supported_vk(keycode: u32) -> bool {
    matches!(
        keycode,
        0x08 | 0x09
            | 0x0d
            | 0x13
            | 0x14
            | 0x1b
            | 0x20..=0x28
            | 0x2d..=0x2e
            | 0x30..=0x39
            | 0x41..=0x5d
            | 0x60..=0x6f
            | 0x70..=0x7e
            | 0x90..=0x91
            | 0xa0..=0xa5
            | 0xba..=0xc0
            | 0xdb..=0xde
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_layout_matches_weston_module_contract() {
        assert_eq!([POINTER_MOTION, POINTER_BUTTON, KEY, SCROLL], [1, 2, 3, 4]);
        let encoded = packet(POINTER_MOTION, 7, 1.5f64.to_bits(), 2.5f64.to_bits(), 3, 4);
        assert_eq!(
            u32::from_le_bytes(encoded[0..4].try_into().unwrap()),
            INPUT_MAGIC
        );
        assert_eq!(u16::from_le_bytes(encoded[4..6].try_into().unwrap()), 1);
        assert_eq!(
            u16::from_le_bytes(encoded[6..8].try_into().unwrap()),
            POINTER_MOTION
        );
        assert_eq!(u64::from_le_bytes(encoded[8..16].try_into().unwrap()), 7);
        assert_eq!(
            f64::from_bits(u64::from_le_bytes(encoded[16..24].try_into().unwrap())),
            1.5
        );
        assert_eq!(
            f64::from_bits(u64::from_le_bytes(encoded[24..32].try_into().unwrap())),
            2.5
        );
    }

    #[test]
    fn canonical_virtual_keys_are_bounded() {
        assert!(supported_vk(0x41));
        assert!(supported_vk(0x7b));
        assert!(supported_vk(0xa2));
        assert!(!supported_vk(0));
        assert!(!supported_vk(u32::MAX));
    }
}
