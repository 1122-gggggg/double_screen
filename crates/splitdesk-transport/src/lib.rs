//! Phase-1 loopback transport plus a WebRTC front that only negotiates
//! capabilities. No webrtc-rs stack is linked.

mod backend;
mod demux;
mod loopback;
mod webrtc;

pub use backend::{TransportBackend, TransportKind};
pub use demux::InputDemux;
pub use loopback::LoopbackTransport;
pub use webrtc::{
    BufferProfile, NegotiatedTransport, NetworkClass, TransportOffer, WebRtcTransport,
    TRANSPORT_PROTOCOL_VERSION,
};

use splitdesk_core::Error;

pub(crate) fn unavailable(detail: impl Into<String>) -> Error {
    Error::BackendUnavailable {
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitdesk_core::Codec;
    use splitdesk_media::{Frame, FramePayload, InputEvent};

    #[test]
    fn loopback_send_recv_encoded_blob() {
        let t = LoopbackTransport::new();
        let blob = vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88];
        let frame = Frame::encoded(42, 16, 16, Codec::H264, blob.clone());
        t.send_media(frame).unwrap();
        let got = t.recv_media().unwrap().expect("encoded frame");
        match got.payload {
            FramePayload::Encoded { codec, bytes } => {
                assert_eq!(codec, Codec::H264);
                assert_eq!(bytes, blob);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
        assert_eq!(got.timestamp_ns, 42);
    }

    #[test]
    fn loopback_pair_forwards_encoded() {
        let (a, b) = LoopbackTransport::pair();
        let blob = vec![9, 8, 7];
        a.send_media(Frame::encoded(1, 8, 8, Codec::H264, blob.clone()))
            .unwrap();
        let got = b.recv_media().unwrap().unwrap();
        assert_eq!(got.encoded_bytes(), Some(blob.as_slice()));
        assert!(a.recv_media().unwrap().is_none());
    }

    #[test]
    fn input_demux_drops_old_motion_keeps_keys() {
        let d = InputDemux::new();
        d.push(InputEvent::PointerMotion {
            x: 1.0,
            y: 1.0,
            ts: 1,
        })
        .unwrap();
        d.push(InputEvent::PointerMotion {
            x: 2.0,
            y: 2.0,
            ts: 2,
        })
        .unwrap();
        d.push(InputEvent::Key {
            keycode: 38,
            pressed: true,
            modifiers: 0,
            ts: 3,
        })
        .unwrap();
        d.push(InputEvent::PointerButton {
            button: 1,
            pressed: true,
            ts: 4,
        })
        .unwrap();
        assert_eq!(d.dropped_latest(), 1);
        let key = d.pop_reliable().unwrap();
        assert!(matches!(key, InputEvent::Key { keycode: 38, .. }));
        let btn = d.pop_reliable().unwrap();
        assert!(matches!(btn, InputEvent::PointerButton { button: 1, .. }));
        match d.take_latest().unwrap() {
            InputEvent::PointerMotion { x, y, ts } => {
                assert_eq!((x, y, ts), (2.0, 2.0, 2));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn webrtc_send_is_unavailable() {
        let mut rtc = WebRtcTransport::lan();
        let offer = rtc.local_offer();
        let negotiated = rtc.negotiate(&offer).unwrap();
        assert_eq!(negotiated.codec, Codec::H264);
        assert_eq!(negotiated.profile.class, NetworkClass::Lan);
        assert!(!negotiated.profile.adaptive);
        let err = rtc
            .send_media(Frame::encoded(0, 1, 1, Codec::H264, vec![1]))
            .unwrap_err();
        assert!(matches!(err, Error::BackendUnavailable { .. }));
    }

    #[test]
    fn wan_profile_is_adaptive() {
        let wan = BufferProfile::wan();
        assert!(wan.adaptive);
        assert!(wan.min_jitter_buffer_ms > BufferProfile::lan().min_jitter_buffer_ms);
    }
}
