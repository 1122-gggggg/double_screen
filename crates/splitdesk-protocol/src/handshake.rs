use serde::{Deserialize, Serialize};
use splitdesk_core::{Capabilities, Codec, Error, SessionId};

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputCaps {
    pub pointer: bool,
    pub keyboard: bool,
    pub scroll: bool,
    pub clipboard: bool,
}

impl Default for InputCaps {
    fn default() -> Self {
        Self {
            pointer: true,
            keyboard: true,
            scroll: true,
            clipboard: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol_versions: Vec<u16>,
    pub codecs: Vec<Codec>,
    pub input_caps: InputCaps,
    pub client_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloAck {
    pub protocol_version: u16,
    pub session_id: SessionId,
    pub capabilities: Capabilities,
    pub selected_codec: Codec,
}

pub fn select_codec(client: &[Codec], server: &[Codec]) -> Result<Codec, Error> {
    for pref in [Codec::H264, Codec::Hevc, Codec::Av1] {
        if client.contains(&pref) && server.contains(&pref) {
            return Ok(pref);
        }
    }
    Err(Error::Unsupported)
}

impl HelloAck {
    pub fn negotiate(
        hello: &Hello,
        session_id: SessionId,
        capabilities: Capabilities,
    ) -> Result<Self, Error> {
        if !hello.protocol_versions.contains(&PROTOCOL_VERSION) {
            return Err(Error::Unsupported);
        }
        let selected_codec = select_codec(&hello.codecs, &capabilities.codecs)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            session_id,
            capabilities,
            selected_codec,
        })
    }
}
