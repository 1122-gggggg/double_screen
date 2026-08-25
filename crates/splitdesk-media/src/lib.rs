//! Host-side media contracts: capture/encode/input traits, a depth-1
//! latest-wins slot, NVENC settings, and an honest GStreamer launch string.
//!
//! This crate does not link GStreamer, NVENC, or a software H.264 encoder.
//! `FramePayload::System` and `SoftwareFallbackEncoder` always mean a CPU copy.

mod frame;
mod input;
mod nvenc;
mod report;
mod slot;
mod software;
mod traits;

pub use frame::{Frame, FramePayload, PixelLayout};
pub use input::{Delivery, InputEvent};
pub use nvenc::{GstNvencPipeline, NvencSettings};
pub use report::MediaPathReport;
pub use slot::BoundedSlot;
pub use software::SoftwareFallbackEncoder;
pub use traits::{
    AudioBackend, AudioPacket, CaptureBackend, CaptureRequest, ClipboardBackend, CursorBackend,
    CursorState, EncoderBackend, InputBackend,
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
    use splitdesk_core::{Codec, EncoderKind, MemoryType};

    #[test]
    fn bounded_slot_drops_old() {
        let slot = BoundedSlot::new();
        assert_eq!(BoundedSlot::<u32>::CAPACITY, 1);
        assert!(slot.push(1).is_none());
        assert_eq!(slot.push(2), Some(1));
        assert_eq!(slot.push(3), Some(2));
        assert_eq!(slot.take(), Some(3));
        assert!(slot.take().is_none());
        assert_eq!(slot.dropped(), 2);
    }

    #[test]
    fn nvenc_settings_zero_b_frames() {
        let settings = NvencSettings::default();
        assert_eq!(settings.b_frames, 0);
        assert!(settings.zerolatency);
        assert_eq!(settings.fps, 60);
        assert_eq!(settings.width, 1920);
        assert_eq!(settings.height, 1080);
        settings.validate().unwrap();
    }

    #[test]
    fn software_fallback_increments_cpu_copies() {
        let mut encoder = SoftwareFallbackEncoder::new();
        assert_eq!(encoder.cpu_copies(), 0);
        let frame = Frame::system(1_000, 2, 2, PixelLayout::Bgra, vec![0u8; 16]);
        let out = encoder.encode(&frame).unwrap().expect("copied frame");
        assert_eq!(encoder.cpu_copies(), 1);
        assert!(encoder.copy_latency_ns().is_some());
        assert!(matches!(out.payload, FramePayload::System { .. }));
        assert_eq!(out.memory, MemoryType::SystemMemory);
        assert_eq!(encoder.kind(), EncoderKind::SoftwareFallback);
        assert!(encoder.memory_path().cpu_copies_per_frame >= 1);
        let _ = encoder.encode(&frame).unwrap();
        assert_eq!(encoder.cpu_copies(), 2);
    }

    #[test]
    fn software_fallback_refuses_gpu_handle() {
        let mut encoder = SoftwareFallbackEncoder::new();
        let frame = Frame::gpu(0, MemoryType::CudaMemory, 64, 64, 7);
        let err = encoder.encode(&frame).unwrap_err();
        assert!(matches!(err, Error::BackendUnavailable { .. }));
        assert_eq!(encoder.cpu_copies(), 0);
    }

    #[test]
    fn gst_pipeline_is_low_latency_h264() {
        let pipe = GstNvencPipeline::new(NvencSettings::default(), MemoryType::CudaMemory);
        let s = pipe.launch_string();
        assert!(s.contains("nvh264enc"));
        assert!(s.contains("bframes=0"));
        assert!(s.contains("zerolatency=true"));
        assert!(s.contains("tune=ultra-low-latency"));
        assert!(s.contains("preset=p1"));
        assert!(s.contains("max-buffers=1"));
        assert!(s.contains("drop=true"));
        assert!(!s.contains("memory:DMABuf"));
        let report = pipe.report();
        assert_eq!(report.codec, Some(Codec::H264));
        assert_eq!(report.path.encoder_input, MemoryType::CudaMemory);
        assert_eq!(report.path.cpu_copies_per_frame, 0);
    }

    #[test]
    fn gst_dmabuf_does_not_claim_direct_nvenc() {
        let pipe = GstNvencPipeline::new(NvencSettings::default(), MemoryType::DmaBuf);
        let s = pipe.parse_launch();
        assert!(s.contains("memory:DMABuf"));
        assert!(s.contains("glupload"));
        assert!(s.contains("memory:GLMemory"));
        let report = pipe.report();
        assert_eq!(report.path.capture, MemoryType::DmaBuf);
        assert_eq!(report.path.conversion, MemoryType::GlMemory);
        assert_eq!(report.path.encoder_input, MemoryType::GlMemory);
        assert_eq!(report.path.cpu_copies_per_frame, 0);
        assert!(!pipe.claims_zero_copy_into_nvenc());
    }

    #[test]
    fn gst_system_memory_marks_cpu_copy() {
        let pipe = GstNvencPipeline::new(NvencSettings::default(), MemoryType::SystemMemory);
        let report = pipe.report();
        assert_eq!(report.path.encoder_input, MemoryType::SystemMemory);
        assert!(report.path.cpu_copies_per_frame >= 1);
        assert!(report.copy_warning);
    }
}
