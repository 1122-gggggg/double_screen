mod auth;
mod clipboard;
mod codec;
mod control;
mod cursor;
mod handshake;
mod input;
mod media;

pub use auth::{unix_now, AuthToken, TokenBinding, TokenStore, AUTH_TOKEN_HEX_LEN};
pub use clipboard::ClipboardUtf8;
pub use codec::{encode_json, try_decode_json, write_json, JsonFrameDecoder, MAX_JSON_FRAME};
pub use control::{Control, DiagnosticsKind};
pub use cursor::{Cursor, Hotspot, Position};
pub use handshake::{select_codec, Hello, HelloAck, InputCaps, PROTOCOL_VERSION};
pub use input::{Input, InputChannels};
pub use media::{
    encode_media_frame, encode_media_frame_header, try_decode_media_frame,
    try_decode_media_frame_bytes, MediaFormat, MediaFrame, MediaHello, MediaHelloAck,
    DEFAULT_MEDIA_BIND, MEDIA_FORMAT_BGRA, MEDIA_FORMAT_H264, MEDIA_HEADER_LEN, MEDIA_MAGIC,
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum Message {
    Hello(Hello),
    HelloAck(HelloAck),
    Control(Control),
    Input(Input),
    Cursor(Cursor),
    Clipboard(ClipboardUtf8),
    Disconnect,
}

pub fn encode_message(msg: &Message) -> Result<Vec<u8>, splitdesk_core::Error> {
    encode_json(msg)
}

pub fn try_decode_message(buf: &[u8]) -> Result<Option<(Message, usize)>, splitdesk_core::Error> {
    try_decode_json(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitdesk_core::{
        Capabilities, Codec, CreateSessionRequest, Error, HostOs, SessionId, UserName,
    };

    #[test]
    fn protocol_roundtrip() {
        let hello = Message::Hello(Hello {
            protocol_versions: vec![PROTOCOL_VERSION],
            codecs: vec![Codec::H264, Codec::Hevc],
            input_caps: InputCaps::default(),
            client_name: "splitdesk-test".into(),
        });
        let bytes = encode_message(&hello).unwrap();
        assert_eq!(
            u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        let (decoded, n) = try_decode_message(&bytes).unwrap().unwrap();
        assert_eq!(n, bytes.len());
        assert_eq!(decoded, hello);

        let control = Message::Control(Control::SessionCreate {
            request: CreateSessionRequest::new("alice"),
        });
        let cbytes = encode_json(&control).unwrap();
        let (cdecoded, _) = try_decode_json::<Message>(&cbytes).unwrap().unwrap();
        assert_eq!(cdecoded, control);

        let motion = Message::Input(Input::PointerMotion {
            x: 12.5,
            y: 4.0,
            ts: 99,
        });
        let mbytes = encode_json(&motion).unwrap();
        let (mdecoded, _) = try_decode_json::<Message>(&mbytes).unwrap().unwrap();
        assert_eq!(mdecoded, motion);
    }

    #[test]
    fn handshake_prefers_h264() {
        let hello = Hello {
            protocol_versions: vec![1],
            codecs: vec![Codec::Av1, Codec::H264],
            input_caps: InputCaps::default(),
            client_name: "c".into(),
        };
        let ack = HelloAck::negotiate(
            &hello,
            SessionId::new(1),
            Capabilities::unprobed(HostOs::Linux),
        )
        .unwrap();
        assert_eq!(ack.protocol_version, PROTOCOL_VERSION);
        assert_eq!(ack.selected_codec, Codec::H264);
        assert_eq!(ack.session_id.to_string(), "sd-001");

        let bad = Hello {
            protocol_versions: vec![99],
            codecs: vec![Codec::H264],
            input_caps: InputCaps::default(),
            client_name: "c".into(),
        };
        assert!(matches!(
            HelloAck::negotiate(
                &bad,
                SessionId::new(1),
                Capabilities::unprobed(HostOs::Linux)
            ),
            Err(Error::Unsupported)
        ));
    }

    #[test]
    fn motion_and_scroll_are_latest_wins() {
        let mut ch = InputChannels::new();
        ch.push(Input::PointerMotion {
            x: 1.0,
            y: 1.0,
            ts: 1,
        })
        .unwrap();
        ch.push(Input::PointerMotion {
            x: 9.0,
            y: 8.0,
            ts: 2,
        })
        .unwrap();
        match ch.take_motion() {
            Some(Input::PointerMotion { x, y, ts }) => {
                assert_eq!((x, y, ts), (9.0, 8.0, 2));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(ch.take_motion().is_none());

        ch.push(Input::Scroll {
            dx: 1.0,
            dy: 0.0,
            ts: 3,
        })
        .unwrap();
        ch.push(Input::Scroll {
            dx: 0.0,
            dy: 4.0,
            ts: 4,
        })
        .unwrap();
        match ch.take_scroll() {
            Some(Input::Scroll { dx, dy, ts }) => {
                assert_eq!((dx, dy, ts), (0.0, 4.0, 4));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn keys_and_buttons_are_reliable_ordered() {
        let mut ch = InputChannels::new();
        ch.push(Input::Key {
            keycode: 10,
            pressed: true,
            modifiers: 0,
            ts: 1,
        })
        .unwrap();
        ch.push(Input::PointerButton {
            button: 1,
            pressed: true,
            ts: 2,
        })
        .unwrap();
        ch.push(Input::Key {
            keycode: 10,
            pressed: false,
            modifiers: 0,
            ts: 3,
        })
        .unwrap();
        match ch.pop_reliable() {
            Some(Input::Key {
                keycode, pressed, ..
            }) => {
                assert_eq!((keycode, pressed), (10, true));
            }
            other => panic!("{other:?}"),
        }
        match ch.pop_reliable() {
            Some(Input::PointerButton {
                button, pressed, ..
            }) => {
                assert_eq!((button, pressed), (1, true));
            }
            other => panic!("{other:?}"),
        }
        let releases = ch.release_held(4);
        assert_eq!(releases.len(), 1);
        assert!(matches!(
            releases[0],
            Input::PointerButton {
                button: 1,
                pressed: false,
                ..
            }
        ));
    }

    #[test]
    fn reliable_queue_is_bounded() {
        let mut ch = InputChannels::with_reliable_cap(2);
        ch.push(Input::Key {
            keycode: 1,
            pressed: true,
            modifiers: 0,
            ts: 1,
        })
        .unwrap();
        ch.push(Input::Key {
            keycode: 2,
            pressed: true,
            modifiers: 0,
            ts: 2,
        })
        .unwrap();
        assert!(matches!(
            ch.push(Input::Key {
                keycode: 3,
                pressed: true,
                modifiers: 0,
                ts: 3,
            }),
            Err(Error::Protocol)
        ));
    }

    #[test]
    fn auth_expires_and_binds() {
        let store = TokenStore::new();
        let user = UserName::new("alice");
        let sid = SessionId::new(1);
        let token = store.issue_at(user.clone(), sid, 1_000, 10);
        assert_eq!(token.as_hex().len(), AUTH_TOKEN_HEX_LEN);
        assert!(!format!("{token:?}").contains(token.as_hex()));
        store.verify_at(&token, &user, &sid, 1_009).unwrap();
        assert!(matches!(
            store.verify_at(&token, &user, &sid, 1_010),
            Err(Error::Auth)
        ));
        let other = UserName::new("bob");
        let token2 = store.issue_at(user.clone(), sid, 2_000, 30);
        assert!(matches!(
            store.verify_at(&token2, &other, &sid, 2_001),
            Err(Error::Auth)
        ));
        assert!(matches!(
            store.verify_at(&token2, &user, &SessionId::new(9), 2_001),
            Err(Error::Auth)
        ));
        store.purge_expired(1_010);
        assert!(matches!(
            store.verify_at(&token, &user, &sid, 1_000),
            Err(Error::Auth)
        ));
    }

    #[test]
    fn clipboard_debug_redacts_text() {
        let clip = ClipboardUtf8::new("super-secret-password");
        let rendered = format!("{clip:?} {clip}");
        assert!(!rendered.contains("super-secret-password"));
        assert!(rendered.contains("len"));
        let msg = Message::Clipboard(clip.clone());
        let bytes = encode_message(&msg).unwrap();
        let (back, _) = try_decode_message(&bytes).unwrap().unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn decoder_waits_for_full_frame() {
        let msg = Message::Disconnect;
        let bytes = encode_message(&msg).unwrap();
        let mut dec = JsonFrameDecoder::new();
        dec.push(&bytes[..3]);
        assert!(dec.next_message::<Message>().unwrap().is_none());
        dec.push(&bytes[3..]);
        assert_eq!(
            dec.next_message::<Message>().unwrap(),
            Some(Message::Disconnect)
        );
    }
}
