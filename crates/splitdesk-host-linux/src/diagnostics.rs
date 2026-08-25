use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
use crate::detect::{nvenc_present, nvidia_present, pipewire_present, run_capture, which};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuDiagnostics {
    pub nvidia: bool,
    pub nvenc: bool,
    pub pipewire: bool,
    pub software_renderer: bool,
    pub renderer: Option<String>,
    pub glxinfo_summary: Option<String>,
    pub vulkaninfo_summary: Option<String>,
    pub nvidia_smi_summary: Option<String>,
    pub notes: Vec<String>,
}

pub fn diagnostics_gpu() -> GpuDiagnostics {
    #[cfg(not(target_os = "linux"))]
    {
        GpuDiagnostics {
            nvidia: false,
            nvenc: false,
            pipewire: false,
            software_renderer: false,
            renderer: None,
            glxinfo_summary: None,
            vulkaninfo_summary: None,
            nvidia_smi_summary: None,
            notes: vec!["linux host gpu diagnostics are not available on this OS".to_string()],
        }
    }

    #[cfg(target_os = "linux")]
    {
        let mut notes = Vec::new();
        let nvidia = nvidia_present();
        let nvenc = nvenc_present();
        let pipewire = pipewire_present();

        let glx = if which("glxinfo").is_some() {
            run_capture("glxinfo", &["-B"]).or_else(|| run_capture("glxinfo", &[]))
        } else {
            notes.push("glxinfo not found".to_string());
            None
        };
        let vk = if which("vulkaninfo").is_some() {
            run_capture("vulkaninfo", &["--summary"]).or_else(|| run_capture("vulkaninfo", &[]))
        } else {
            notes.push("vulkaninfo not found".to_string());
            None
        };
        let smi = if which("nvidia-smi").is_some() {
            run_capture(
                "nvidia-smi",
                &[
                    "--query-gpu=name,driver_version,uuid",
                    "--format=csv,noheader",
                ],
            )
            .or_else(|| run_capture("nvidia-smi", &["-L"]))
        } else {
            if nvidia {
                notes.push("nvidia device nodes present but nvidia-smi not found".to_string());
            }
            None
        };

        let renderer = first_renderer(glx.as_deref(), vk.as_deref());
        let software_renderer =
            is_software_renderer(renderer.as_deref(), glx.as_deref(), vk.as_deref());
        if software_renderer {
            notes.push(
                "llvmpipe/software renderer detected; GPU encode/capture path is not available"
                    .to_string(),
            );
        }
        if nvidia && !nvenc {
            notes.push(
                "NVIDIA GPU present but NVENC was not confirmed via gst-inspect/ffmpeg/nvidia-smi"
                    .to_string(),
            );
        }

        GpuDiagnostics {
            nvidia,
            nvenc,
            pipewire,
            software_renderer,
            renderer,
            glxinfo_summary: glx.map(summarize),
            vulkaninfo_summary: vk.map(summarize),
            nvidia_smi_summary: smi.map(summarize),
            notes,
        }
    }
}

#[cfg(target_os = "linux")]
fn first_renderer(glx: Option<&str>, vk: Option<&str>) -> Option<String> {
    if let Some(glx) = glx {
        for line in glx.lines() {
            if let Some(rest) = line.split_once(':') {
                let key = rest.0.to_ascii_lowercase();
                if key.contains("opengl renderer") {
                    return Some(rest.1.trim().to_string());
                }
            }
        }
    }
    if let Some(vk) = vk {
        for line in vk.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.contains("devicename") || lower.contains("device name") {
                if let Some((_, v)) = line.split_once('=') {
                    return Some(v.trim().trim_matches('"').to_string());
                }
                if let Some((_, v)) = line.split_once(':') {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn is_software_renderer(renderer: Option<&str>, glx: Option<&str>, vk: Option<&str>) -> bool {
    let blob = format!(
        "{} {} {}",
        renderer.unwrap_or(""),
        glx.unwrap_or(""),
        vk.unwrap_or("")
    )
    .to_ascii_lowercase();
    blob.contains("llvmpipe")
        || blob.contains("softpipe")
        || blob.contains("swrast")
        || blob.contains("swiftshader")
}

#[cfg(target_os = "linux")]
fn summarize(text: String) -> String {
    let mut out = String::new();
    for line in text.lines().take(24) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out
}
