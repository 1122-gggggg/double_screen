use crate::backend::{webrtc_not_linked, TransportBackend, TransportKind};
use serde::{Deserialize, Serialize};
use splitdesk_core::{Codec, Error};
use splitdesk_media::{Frame, InputEvent};

pub const TRANSPORT_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkClass {
    Lan,
    Wan,
}

/// LAN uses the smallest jitter buffer. WAN is adaptive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferProfile {
    pub class: NetworkClass,
    pub min_jitter_buffer_ms: u32,
    pub max_jitter_buffer_ms: u32,
    pub adaptive: bool,
}

impl BufferProfile {
    pub fn lan() -> Self {
        Self {
            class: NetworkClass::Lan,
            min_jitter_buffer_ms: 0,
            max_jitter_buffer_ms: 8,
            adaptive: false,
        }
    }

    pub fn wan() -> Self {
        Self {
            class: NetworkClass::Wan,
            min_jitter_buffer_ms: 15,
            max_jitter_buffer_ms: 80,
            adaptive: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportOffer {
    pub protocol_versions: Vec<u16>,
    pub codecs: Vec<Codec>,
    pub profile: BufferProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedTransport {
    pub protocol_version: u16,
    pub codec: Codec,
    pub profile: BufferProfile,
}

/// Capability negotiation only. Send/recv return [`Error::BackendUnavailable`].
pub struct WebRtcTransport {
    profile: BufferProfile,
    local_codecs: Vec<Codec>,
    negotiated: Option<NegotiatedTransport>,
}

impl WebRtcTransport {
    pub fn new(profile: BufferProfile) -> Self {
        Self {
            profile,
            local_codecs: vec![Codec::H264, Codec::Hevc, Codec::Av1],
            negotiated: None,
        }
    }

    pub fn lan() -> Self {
        Self::new(BufferProfile::lan())
    }

    pub fn wan() -> Self {
        Self::new(BufferProfile::wan())
    }

    pub fn profile(&self) -> &BufferProfile {
        &self.profile
    }

    pub fn negotiated(&self) -> Option<&NegotiatedTransport> {
        self.negotiated.as_ref()
    }

    pub fn local_offer(&self) -> TransportOffer {
        TransportOffer {
            protocol_versions: vec![TRANSPORT_PROTOCOL_VERSION],
            codecs: self.local_codecs.clone(),
            profile: self.profile.clone(),
        }
    }

    pub fn negotiate(&mut self, remote: &TransportOffer) -> Result<NegotiatedTransport, Error> {
        if !remote
            .protocol_versions
            .contains(&TRANSPORT_PROTOCOL_VERSION)
        {
            return Err(Error::Protocol);
        }
        let codec = prefer_codec(&self.local_codecs, &remote.codecs).ok_or(Error::Unsupported)?;
        let profile = merge_profiles(&self.profile, &remote.profile);
        let selected = NegotiatedTransport {
            protocol_version: TRANSPORT_PROTOCOL_VERSION,
            codec,
            profile,
        };
        self.negotiated = Some(selected.clone());
        Ok(selected)
    }
}

fn prefer_codec(local: &[Codec], remote: &[Codec]) -> Option<Codec> {
    const ORDER: [Codec; 3] = [Codec::H264, Codec::Hevc, Codec::Av1];
    ORDER
        .into_iter()
        .find(|codec| local.contains(codec) && remote.contains(codec))
}

fn merge_profiles(local: &BufferProfile, remote: &BufferProfile) -> BufferProfile {
    if local.class == NetworkClass::Wan || remote.class == NetworkClass::Wan {
        let mut wan = BufferProfile::wan();
        wan.min_jitter_buffer_ms = local
            .min_jitter_buffer_ms
            .max(remote.min_jitter_buffer_ms)
            .max(wan.min_jitter_buffer_ms);
        wan.max_jitter_buffer_ms = local
            .max_jitter_buffer_ms
            .max(remote.max_jitter_buffer_ms)
            .max(wan.max_jitter_buffer_ms);
        wan
    } else {
        BufferProfile::lan()
    }
}

impl TransportBackend for WebRtcTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::WebRtc
    }

    fn send_media(&self, _frame: Frame) -> Result<(), Error> {
        Err(webrtc_not_linked())
    }

    fn recv_media(&self) -> Result<Option<Frame>, Error> {
        Err(webrtc_not_linked())
    }

    fn send_input(&self, _event: InputEvent) -> Result<(), Error> {
        Err(webrtc_not_linked())
    }

    fn recv_input(&self) -> Result<Option<InputEvent>, Error> {
        Err(webrtc_not_linked())
    }

    fn send_control(&self, _bytes: Vec<u8>) -> Result<(), Error> {
        Err(webrtc_not_linked())
    }

    fn recv_control(&self) -> Result<Option<Vec<u8>>, Error> {
        Err(webrtc_not_linked())
    }

    fn close(&self) -> Result<(), Error> {
        Ok(())
    }
}
