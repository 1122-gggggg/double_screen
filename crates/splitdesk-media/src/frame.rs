use serde::{Deserialize, Serialize};
use splitdesk_core::{Codec, MemoryType};
use std::fmt;

/// Pixel layout stored in [`FramePayload::System`]. The field name on the
/// payload is `nv12_or_bgra` as required by the SplitDesk contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PixelLayout {
    Nv12,
    Bgra,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FramePayload {
    GpuHandle {
        id: u64,
    },
    Encoded {
        codec: Codec,
        bytes: Vec<u8>,
    },
    System {
        nv12_or_bgra: PixelLayout,
        bytes: Vec<u8>,
    },
}

impl fmt::Debug for FramePayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GpuHandle { id } => f.debug_struct("GpuHandle").field("id", id).finish(),
            Self::Encoded { codec, bytes } => f
                .debug_struct("Encoded")
                .field("codec", codec)
                .field("len", &bytes.len())
                .finish(),
            Self::System {
                nv12_or_bgra,
                bytes,
            } => f
                .debug_struct("System")
                .field("nv12_or_bgra", nv12_or_bgra)
                .field("len", &bytes.len())
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub timestamp_ns: u64,
    pub memory: MemoryType,
    pub width: u32,
    pub height: u32,
    pub payload: FramePayload,
}

impl Frame {
    pub fn gpu(timestamp_ns: u64, memory: MemoryType, width: u32, height: u32, id: u64) -> Self {
        Self {
            timestamp_ns,
            memory,
            width,
            height,
            payload: FramePayload::GpuHandle { id },
        }
    }

    pub fn encoded(
        timestamp_ns: u64,
        width: u32,
        height: u32,
        codec: Codec,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            timestamp_ns,
            memory: MemoryType::SystemMemory,
            width,
            height,
            payload: FramePayload::Encoded { codec, bytes },
        }
    }

    /// System-memory pixels. Always a CPU-resident path; logs a warning and
    /// does not claim GPU residency or zero-copy.
    pub fn system(
        timestamp_ns: u64,
        width: u32,
        height: u32,
        nv12_or_bgra: PixelLayout,
        bytes: Vec<u8>,
    ) -> Self {
        tracing::warn!(
            width,
            height,
            layout = ?nv12_or_bgra,
            bytes = bytes.len(),
            "FramePayload::System is CPU memory; this is not a zero-copy GPU path"
        );
        Self {
            timestamp_ns,
            memory: MemoryType::SystemMemory,
            width,
            height,
            payload: FramePayload::System {
                nv12_or_bgra,
                bytes,
            },
        }
    }

    pub fn encoded_bytes(&self) -> Option<&[u8]> {
        match &self.payload {
            FramePayload::Encoded { bytes, .. } => Some(bytes),
            _ => None,
        }
    }
}
