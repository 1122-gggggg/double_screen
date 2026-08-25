use crate::frame::{Frame, FramePayload, PixelLayout};
use crate::nvenc::NvencSettings;
use crate::report::MediaPathReport;
use crate::traits::EncoderBackend;
use crate::unavailable;
use splitdesk_core::{CaptureKind, EncoderKind, Error, MemoryPath, MemoryType};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// CPU-memory encoder stand-in. Copies pixels into `FramePayload::System`,
/// records `copy_latency_ns`, and does not emit H.264.
pub struct SoftwareFallbackEncoder {
    settings: NvencSettings,
    cpu_copies: AtomicU32,
    copy_latency_ns: AtomicU64,
    has_copy: AtomicU32,
}

impl Default for SoftwareFallbackEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftwareFallbackEncoder {
    pub fn new() -> Self {
        Self {
            settings: NvencSettings::default(),
            cpu_copies: AtomicU32::new(0),
            copy_latency_ns: AtomicU64::new(0),
            has_copy: AtomicU32::new(0),
        }
    }

    pub fn cpu_copies(&self) -> u32 {
        self.cpu_copies.load(Ordering::Relaxed)
    }

    pub fn settings(&self) -> &NvencSettings {
        &self.settings
    }

    pub fn expected_layout(width: u32, height: u32, layout: PixelLayout) -> usize {
        match layout {
            PixelLayout::Bgra => (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(4),
            PixelLayout::Nv12 => {
                let y = (width as usize).saturating_mul(height as usize);
                y.saturating_add(y / 2)
            }
        }
    }

    fn record_copy(&self, latency_ns: u64, bytes: usize) {
        self.copy_latency_ns.store(latency_ns, Ordering::Relaxed);
        self.has_copy.store(1, Ordering::Relaxed);
        let n = self.cpu_copies.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(
            copy_latency_ns = latency_ns,
            cpu_copies = n,
            bytes,
            "SoftwareFallbackEncoder performed a CPU copy; this is not a zero-copy path"
        );
    }
}

impl EncoderBackend for SoftwareFallbackEncoder {
    fn kind(&self) -> EncoderKind {
        EncoderKind::SoftwareFallback
    }

    fn configure(&mut self, settings: &NvencSettings) -> Result<(), Error> {
        settings.validate()?;
        self.settings = settings.clone();
        Ok(())
    }

    fn encode(&mut self, frame: &Frame) -> Result<Option<Frame>, Error> {
        match &frame.payload {
            FramePayload::GpuHandle { id } => Err(unavailable(format!(
                "SoftwareFallbackEncoder cannot read GPU handle {id} ({:?}); supply System pixels",
                frame.memory
            ))),
            FramePayload::Encoded { codec, .. } => Err(unavailable(format!(
                "SoftwareFallbackEncoder does not transcode encoded {codec:?} bitstreams"
            ))),
            FramePayload::System {
                nv12_or_bgra,
                bytes,
            } => {
                let start = Instant::now();
                let copied = bytes.clone();
                let latency_ns = start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
                self.record_copy(latency_ns, copied.len());
                Ok(Some(Frame {
                    timestamp_ns: frame.timestamp_ns,
                    memory: MemoryType::SystemMemory,
                    width: frame.width,
                    height: frame.height,
                    payload: FramePayload::System {
                        nv12_or_bgra: *nv12_or_bgra,
                        bytes: copied,
                    },
                }))
            }
        }
    }

    fn force_idr(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn memory_path(&self) -> MemoryPath {
        MemoryPath::system_copies(1)
    }

    fn copy_latency_ns(&self) -> Option<u64> {
        if self.has_copy.load(Ordering::Relaxed) == 0 {
            None
        } else {
            Some(self.copy_latency_ns.load(Ordering::Relaxed))
        }
    }

    fn report(&self) -> MediaPathReport {
        MediaPathReport {
            path: self.memory_path(),
            capture: CaptureKind::Unavailable,
            encoder: EncoderKind::SoftwareFallback,
            codec: None,
            copy_latency_ns: self.copy_latency_ns(),
            copy_warning: true,
            frames_copied: u64::from(self.cpu_copies()),
            notes: vec![
                "SoftwareFallbackEncoder copies CPU pixels and does not produce H.264".into(),
            ],
        }
    }
}
