use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryType {
    DmaBuf,
    GlMemory,
    CudaMemory,
    D3D11,
    D3D12,
    SystemMemory,
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MemoryType::DmaBuf => "DmaBuf",
            MemoryType::GlMemory => "GlMemory",
            MemoryType::CudaMemory => "CudaMemory",
            MemoryType::D3D11 => "D3D11",
            MemoryType::D3D12 => "D3D12",
            MemoryType::SystemMemory => "SystemMemory",
        })
    }
}

impl MemoryType {
    pub fn is_gpu_native(self) -> bool {
        !matches!(self, MemoryType::SystemMemory)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryPath {
    pub render_output: MemoryType,
    pub capture: MemoryType,
    pub conversion: MemoryType,
    pub encoder_input: MemoryType,
    pub cpu_copies_per_frame: u32,
}

impl MemoryPath {
    pub fn system_copies(cpu_copies_per_frame: u32) -> Self {
        Self {
            render_output: MemoryType::SystemMemory,
            capture: MemoryType::SystemMemory,
            conversion: MemoryType::SystemMemory,
            encoder_input: MemoryType::SystemMemory,
            cpu_copies_per_frame,
        }
    }

    pub fn describes_zero_copy(self) -> bool {
        self.cpu_copies_per_frame == 0
            && self.render_output.is_gpu_native()
            && self.capture.is_gpu_native()
            && self.conversion.is_gpu_native()
            && self.encoder_input.is_gpu_native()
    }
}

impl fmt::Display for MemoryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "render_output={} capture={} conversion={} encoder_input={} cpu_copies_per_frame={}",
            self.render_output,
            self.capture,
            self.conversion,
            self.encoder_input,
            self.cpu_copies_per_frame
        )
    }
}
