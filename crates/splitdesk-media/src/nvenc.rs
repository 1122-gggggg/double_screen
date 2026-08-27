use crate::report::MediaPathReport;
use crate::unavailable;
use serde::{Deserialize, Serialize};
use splitdesk_core::{CaptureKind, Codec, EncoderKind, Error, MemoryPath, MemoryType};
use std::process::Command;

/// Low-latency NVENC knobs. `b_frames` must stay 0; `zerolatency` must stay true.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvencSettings {
    pub zerolatency: bool,
    pub b_frames: u32,
    pub gop: u32,
    pub bitrate: u32,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for NvencSettings {
    fn default() -> Self {
        Self::low_latency(1920, 1080, 60, 8_000)
    }
}

impl NvencSettings {
    /// `bitrate` is kilobits per second (GStreamer `nvh264enc` units).
    /// `gop == 0` means infinite GOP (`gop-size=-1`).
    pub fn low_latency(width: u32, height: u32, fps: u32, bitrate: u32) -> Self {
        Self {
            zerolatency: true,
            b_frames: 0,
            gop: 0,
            bitrate,
            fps,
            width,
            height,
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        if !self.zerolatency {
            return Err(unavailable(
                "NvencSettings.zerolatency must be true (reordering adds latency)",
            ));
        }
        if self.b_frames != 0 {
            return Err(unavailable(
                "NvencSettings.b_frames must be 0 (B-frames add reorder delay)",
            ));
        }
        if self.width == 0 || self.height == 0 || self.fps == 0 {
            return Err(unavailable(
                "NvencSettings width, height, and fps must be non-zero",
            ));
        }
        if self.bitrate == 0 {
            return Err(unavailable("NvencSettings.bitrate must be non-zero"));
        }
        Ok(())
    }

    pub fn gop_size_gst(&self) -> i32 {
        if self.gop == 0 {
            -1
        } else {
            self.gop as i32
        }
    }

    /// About one encoded frame of VBV, in kbits.
    pub fn vbv_buffer_kbits(&self) -> u32 {
        let fps = self.fps.max(1);
        (self.bitrate / fps).max(1)
    }
}

/// Builds a `gst-launch-1.0` / `gst_parse_launch` string for low-latency H.264.
/// Does not link GStreamer and does not spawn an encoder.
///
/// Official `nvh264enc` sink: CUDAMemory, D3D12Memory, GLMemory, system
/// `video/x-raw`. DMA-BUF is not in the pad template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GstNvencPipeline {
    pub settings: NvencSettings,
    pub input_memory: MemoryType,
}

impl GstNvencPipeline {
    pub fn new(settings: NvencSettings, input_memory: MemoryType) -> Self {
        Self {
            settings,
            input_memory,
        }
    }

    /// `gst-inspect-1.0 nvh264enc` (or `gst-inspect`) succeeds.
    pub fn available() -> bool {
        const TOOLS: &[&str] = &["gst-inspect-1.0", "gst-inspect"];
        for tool in TOOLS {
            match Command::new(tool).arg("nvh264enc").output() {
                Ok(out) if out.status.success() => return true,
                _ => continue,
            }
        }
        false
    }

    pub fn codec(&self) -> Codec {
        Codec::H264
    }

    pub fn encoder_element(&self) -> &'static str {
        match self.input_memory {
            MemoryType::D3D11 => "nvd3d11h264enc",
            _ => "nvh264enc",
        }
    }

    /// Direct NVENC sink only for CUDA / GL / D3D12. DMA-BUF is not claimed.
    pub fn claims_zero_copy_into_nvenc(&self) -> bool {
        matches!(
            self.input_memory,
            MemoryType::CudaMemory | MemoryType::GlMemory | MemoryType::D3D12
        )
    }

    pub fn parse_launch(&self) -> String {
        let s = &self.settings;
        let caps = raw_caps(self.input_memory, s.width, s.height, s.fps);
        let hop = conversion_hop(self.input_memory);
        let enc = self.encoder_element();
        let enc_props = format!(
            "{enc} name=sd_enc zerolatency=true bframes=0 bitrate={} gop-size={} \
             rc-mode=cbr preset=p1 tune=ultra-low-latency rc-lookahead=0 \
             b-adapt=false multi-pass=disabled vbv-buffer-size={}",
            s.bitrate,
            s.gop_size_gst(),
            s.vbv_buffer_kbits()
        );
        format!(
            "appsrc name=sd_src is-live=true format=time do-timestamp=true block=false \
             max-bytes=0 emit-signals=false ! {caps}{hop} ! {enc_props} ! \
             video/x-h264,stream-format=byte-stream,alignment=au ! \
             appsink name=sd_sink sync=false async=false max-buffers=1 drop=true qos=true"
        )
    }

    pub fn launch_string(&self) -> String {
        format!("gst-launch-1.0 -q {}", self.parse_launch())
    }

    pub fn launch_argv(&self) -> Vec<String> {
        vec![
            "gst-launch-1.0".to_string(),
            "-q".to_string(),
            self.parse_launch(),
        ]
    }

    /// PipeWire capture pipeline used by a headless Weston session.
    ///
    /// The PipeWire DMA-BUF is imported through GL because `nvh264enc` does
    /// not advertise DMA-BUF on its sink. Every output access unit is an IDR
    /// with repeated SPS/PPS: the daemon's depth-1 relay can therefore drop an
    /// old access unit without corrupting the next one.
    pub fn weston_pipewire_parse_launch(&self, target_object: &str) -> Result<String, Error> {
        self.settings.validate()?;
        if target_object.is_empty()
            || !target_object
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(unavailable("invalid PipeWire target object"));
        }
        let s = &self.settings;
        Ok(format!(
            "pipewiresrc target-object={target_object} do-timestamp=true ! \
             queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! \
             video/x-raw(memory:DMABuf) ! \
             glupload ! glcolorconvert ! \
             video/x-raw(memory:GLMemory),format=NV12,width={},height={},framerate={}/1 ! \
             nvh264enc name=sd_enc zerolatency=true bframes=0 bitrate={} gop-size=1 \
             aud=true repeat-sequence-header=true rc-mode=cbr preset=p1 \
             tune=ultra-low-latency rc-lookahead=0 b-adapt=false multi-pass=disabled \
             vbv-buffer-size={} ! h264parse config-interval=-1 disable-passthrough=true ! \
             video/x-h264,profile=constrained-baseline,stream-format=byte-stream,alignment=au ! \
             fdsink fd=1 sync=false async=false",
            s.width,
            s.height,
            s.fps,
            s.bitrate,
            s.vbv_buffer_kbits(),
        ))
    }

    pub fn weston_pipewire_argv(&self, target_object: &str) -> Result<Vec<String>, Error> {
        let launch = self.weston_pipewire_parse_launch(target_object)?;
        let mut argv = vec!["gst-launch-1.0".to_string(), "-q".to_string()];
        argv.extend(launch.split_ascii_whitespace().map(str::to_string));
        Ok(argv)
    }

    pub fn memory_path(&self) -> MemoryPath {
        path_for_input(self.input_memory)
    }

    pub fn report(&self) -> MediaPathReport {
        let path = self.memory_path();
        let mut notes = Vec::new();
        match self.input_memory {
            MemoryType::DmaBuf => notes.push(
                "nvh264enc does not accept memory:DMABuf; pipeline imports via glupload to GLMemory"
                    .into(),
            ),
            MemoryType::D3D11 => notes.push(
                "nvh264enc is CUDA-mode and does not list D3D11Memory; using nvd3d11h264enc"
                    .into(),
            ),
            MemoryType::SystemMemory => notes.push(
                "system video/x-raw is accepted by nvh264enc but requires a CPU upload".into(),
            ),
            MemoryType::CudaMemory | MemoryType::GlMemory | MemoryType::D3D12 => notes.push(
                "encoder sink lists this GPU memory feature; host must still import into it"
                    .into(),
            ),
        }
        MediaPathReport {
            path,
            capture: capture_hint(self.input_memory),
            encoder: EncoderKind::Nvenc,
            codec: Some(self.codec()),
            copy_latency_ns: None,
            copy_warning: path.cpu_copies_per_frame > 0,
            frames_copied: 0,
            notes,
        }
    }
}

fn raw_caps(memory: MemoryType, width: u32, height: u32, fps: u32) -> String {
    let feature = match memory {
        MemoryType::DmaBuf => Some("memory:DMABuf"),
        MemoryType::GlMemory => Some("memory:GLMemory"),
        MemoryType::CudaMemory => Some("memory:CUDAMemory"),
        MemoryType::D3D11 => Some("memory:D3D11Memory"),
        MemoryType::D3D12 => Some("memory:D3D12Memory"),
        MemoryType::SystemMemory => None,
    };
    let video = match feature {
        Some(feat) => format!("video/x-raw({feat})"),
        None => "video/x-raw".to_string(),
    };
    format!("{video},format=NV12,width={width},height={height},framerate={fps}/1")
}

fn conversion_hop(memory: MemoryType) -> &'static str {
    match memory {
        MemoryType::DmaBuf => {
            " ! glupload ! glcolorconvert ! video/x-raw(memory:GLMemory),format=NV12"
        }
        MemoryType::CudaMemory
        | MemoryType::GlMemory
        | MemoryType::D3D12
        | MemoryType::D3D11
        | MemoryType::SystemMemory => "",
    }
}

fn path_for_input(memory: MemoryType) -> MemoryPath {
    match memory {
        MemoryType::CudaMemory => MemoryPath {
            render_output: MemoryType::CudaMemory,
            capture: MemoryType::CudaMemory,
            conversion: MemoryType::CudaMemory,
            encoder_input: MemoryType::CudaMemory,
            cpu_copies_per_frame: 0,
        },
        MemoryType::GlMemory => MemoryPath {
            render_output: MemoryType::GlMemory,
            capture: MemoryType::GlMemory,
            conversion: MemoryType::GlMemory,
            encoder_input: MemoryType::GlMemory,
            cpu_copies_per_frame: 0,
        },
        MemoryType::D3D12 => MemoryPath {
            render_output: MemoryType::D3D12,
            capture: MemoryType::D3D12,
            conversion: MemoryType::D3D12,
            encoder_input: MemoryType::D3D12,
            cpu_copies_per_frame: 0,
        },
        MemoryType::D3D11 => MemoryPath {
            render_output: MemoryType::D3D11,
            capture: MemoryType::D3D11,
            conversion: MemoryType::D3D11,
            encoder_input: MemoryType::D3D11,
            cpu_copies_per_frame: 0,
        },
        MemoryType::DmaBuf => MemoryPath {
            render_output: MemoryType::DmaBuf,
            capture: MemoryType::DmaBuf,
            conversion: MemoryType::GlMemory,
            encoder_input: MemoryType::GlMemory,
            cpu_copies_per_frame: 0,
        },
        MemoryType::SystemMemory => MemoryPath {
            render_output: MemoryType::SystemMemory,
            capture: MemoryType::SystemMemory,
            conversion: MemoryType::SystemMemory,
            encoder_input: MemoryType::SystemMemory,
            cpu_copies_per_frame: 1,
        },
    }
}

fn capture_hint(memory: MemoryType) -> CaptureKind {
    match memory {
        MemoryType::DmaBuf | MemoryType::GlMemory => CaptureKind::PipeWire,
        MemoryType::D3D11 => CaptureKind::Dxgi,
        MemoryType::D3D12 => CaptureKind::WindowsGraphicsCapture,
        MemoryType::CudaMemory | MemoryType::SystemMemory => CaptureKind::Unavailable,
    }
}
