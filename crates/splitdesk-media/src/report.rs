use serde::{Deserialize, Serialize};
use splitdesk_core::{CaptureKind, Codec, EncoderKind, MemoryPath};
use std::fmt;

/// Printed by `splitdesk diagnostics media-path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPathReport {
    pub path: MemoryPath,
    pub capture: CaptureKind,
    pub encoder: EncoderKind,
    pub codec: Option<Codec>,
    pub copy_latency_ns: Option<u64>,
    pub copy_warning: bool,
    pub frames_copied: u64,
    pub notes: Vec<String>,
}

impl MediaPathReport {
    pub fn render(&self) -> String {
        let codec = match self.codec {
            Some(c) => format!("{c:?}"),
            None => "none".into(),
        };
        let copy_ns = match self.copy_latency_ns {
            Some(ns) => format!("{ns}"),
            None => "n/a".into(),
        };
        let mut out = format!(
            "media-path\n  render_output: {:?}\n  capture: {:?}\n  conversion: {:?}\n  \
             encoder_input: {:?}\n  cpu_copies_per_frame: {}\n  capture_kind: {:?}\n  \
             encoder_kind: {:?}\n  codec: {codec}\n  copy_latency_ns: {copy_ns}\n  \
             copy_warning: {}\n  frames_copied: {}",
            self.path.render_output,
            self.path.capture,
            self.path.conversion,
            self.path.encoder_input,
            self.path.cpu_copies_per_frame,
            self.capture,
            self.encoder,
            self.copy_warning,
            self.frames_copied
        );
        for note in &self.notes {
            out.push_str("\n  note: ");
            out.push_str(note);
        }
        out
    }
}

impl fmt::Display for MediaPathReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}
