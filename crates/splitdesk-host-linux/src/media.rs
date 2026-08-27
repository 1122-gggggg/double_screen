use std::io::{self, Read};

use splitdesk_core::Error;
#[cfg(target_os = "linux")]
use splitdesk_core::MemoryType;
#[cfg(target_os = "linux")]
use splitdesk_media::{GstNvencPipeline, NvencSettings};
#[cfg(target_os = "linux")]
use splitdesk_protocol::MediaFrame;

const MAX_ACCESS_UNIT: usize = 16 * 1024 * 1024;

pub struct AnnexBAccessUnitReader<R> {
    reader: R,
    buffered: Vec<u8>,
    eof: bool,
}

impl<R: Read> AnnexBAccessUnitReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffered: Vec::with_capacity(256 * 1024),
            eof: false,
        }
    }

    /// Returns one Annex-B access unit delimited by H.264 AUD NAL units.
    /// `nvh264enc aud=true` and `h264parse alignment=au` make this boundary
    /// explicit even though a pipe does not preserve GStreamer buffer writes.
    pub fn next_access_unit(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let first = find_nal(&self.buffered, 0, 9);
            let second = first.and_then(|first| find_nal(&self.buffered, first + 4, 9));
            if let Some(boundary) = second {
                let tail = self.buffered.split_off(boundary);
                let access_unit = std::mem::replace(&mut self.buffered, tail);
                validate_independent_access_unit(&access_unit)?;
                return Ok(Some(access_unit));
            }
            if self.eof {
                if self.buffered.is_empty() || first.is_none() {
                    self.buffered.clear();
                    return Ok(None);
                }
                let access_unit = std::mem::take(&mut self.buffered);
                validate_independent_access_unit(&access_unit)?;
                return Ok(Some(access_unit));
            }
            if self.buffered.len() >= MAX_ACCESS_UNIT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "H.264 access unit exceeds 16 MiB",
                ));
            }
            let mut chunk = [0u8; 64 * 1024];
            match self.reader.read(&mut chunk)? {
                0 => self.eof = true,
                count => self.buffered.extend_from_slice(&chunk[..count]),
            }
        }
    }
}

fn validate_independent_access_unit(access_unit: &[u8]) -> io::Result<()> {
    if access_unit.len() > MAX_ACCESS_UNIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "H.264 access unit exceeds 16 MiB",
        ));
    }
    if find_nal(access_unit, 0, 9).is_none()
        || find_nal(access_unit, 0, 7).is_none()
        || find_nal(access_unit, 0, 8).is_none()
        || find_nal(access_unit, 0, 5).is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NVENC output is not an independent AUD/SPS/PPS/IDR access unit",
        ));
    }
    Ok(())
}

fn find_nal(bytes: &[u8], from: usize, wanted_type: u8) -> Option<usize> {
    let mut index = from;
    while index + 4 <= bytes.len() {
        let (start, header) = if bytes[index..].starts_with(&[0, 0, 1]) {
            (index, index + 3)
        } else if index + 5 <= bytes.len() && bytes[index..].starts_with(&[0, 0, 0, 1]) {
            (index, index + 4)
        } else {
            index += 1;
            continue;
        };
        if bytes.get(header).copied().map(|byte| byte & 0x1f) == Some(wanted_type) {
            return Some(start);
        }
        index = header + 1;
    }
    None
}

#[cfg(target_os = "linux")]
pub struct PipeWireNvencCapture {
    child: std::process::Child,
    access_units: AnnexBAccessUnitReader<std::process::ChildStdout>,
    width: u32,
    height: u32,
}

#[cfg(not(target_os = "linux"))]
pub struct PipeWireNvencCapture;

impl PipeWireNvencCapture {
    #[cfg(target_os = "linux")]
    pub fn start(runtime: &crate::backend::LinuxSessionRuntime) -> Result<Self, Error> {
        use std::ffi::CString;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let gst = crate::detect::which("gst-launch-1.0")
            .ok_or_else(|| Error::backend("gst-launch-1.0 not found"))?;
        let settings = NvencSettings::low_latency(
            runtime.resolution.width,
            runtime.resolution.height,
            runtime.fps,
            8_000,
        );
        let pipeline = GstNvencPipeline::new(settings, MemoryType::DmaBuf);
        let argv = pipeline.weston_pipewire_argv(&runtime.pipewire_target)?;
        let mut command = Command::new(gst);
        command.args(&argv[1..]);
        command.env_clear();
        command.env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
        );
        command.env("HOME", &runtime.home);
        command.env("USER", &runtime.user);
        command.env("LOGNAME", &runtime.user);
        command.env("XDG_RUNTIME_DIR", &runtime.runtime_dir);
        command.env("PIPEWIRE_REMOTE", "pipewire-0");
        command.env("GST_GL_PLATFORM", "egl");
        command.env("GST_GL_WINDOW", "surfaceless");
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::inherit());

        if unsafe { libc::geteuid() } == 0 {
            let user = CString::new(runtime.user.as_str())
                .map_err(|_| Error::backend("session user contains NUL"))?;
            let uid = runtime.uid;
            let gid = runtime.gid;
            unsafe {
                command.pre_exec(move || {
                    if libc::initgroups(user.as_ptr(), gid) != 0
                        || libc::setgid(gid) != 0
                        || libc::setuid(uid) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let mut child = command.spawn().map_err(|error| {
            Error::backend(format!("failed to start PipeWire/NVENC pipeline: {error}"))
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::backend("NVENC pipeline stdout was not piped"))?;
        Ok(Self {
            child,
            access_units: AnnexBAccessUnitReader::new(stdout),
            width: runtime.resolution.width,
            height: runtime.resolution.height,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn start(_runtime: &crate::backend::LinuxSessionRuntime) -> Result<Self, Error> {
        Err(Error::backend(
            "PipeWire/NVENC capture is only available on Linux",
        ))
    }

    #[cfg(target_os = "linux")]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    #[cfg(not(target_os = "linux"))]
    pub fn pid(&self) -> u32 {
        0
    }

    #[cfg(target_os = "linux")]
    pub fn next_frame(&mut self) -> Result<Option<MediaFrame>, Error> {
        let access_unit = self
            .access_units
            .next_access_unit()
            .map_err(|error| Error::backend(format!("read NVENC output: {error}")))?;
        let Some(access_unit) = access_unit else {
            let status = self
                .child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.to_string())
                .unwrap_or_else(|| "closed stdout".to_string());
            return Err(Error::backend(format!(
                "PipeWire/NVENC pipeline stopped: {status}"
            )));
        };
        let timestamp_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        Ok(Some(MediaFrame::h264(
            self.width,
            self.height,
            timestamp_ns,
            access_unit,
        )))
    }
}

#[cfg(target_os = "linux")]
impl Drop for PipeWireNvencCapture {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn independent(seed: u8) -> Vec<u8> {
        vec![
            0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 7, seed, 0, 0, 0, 1, 8, seed, 0, 0, 0, 1, 5, seed,
        ]
    }

    #[test]
    fn annex_b_reader_recovers_pipe_boundaries() {
        let first = independent(1);
        let second = independent(2);
        let mut stream = vec![0, 0, 0, 1, 7, 0];
        stream.extend_from_slice(&first);
        stream.extend_from_slice(&second);
        let mut reader = AnnexBAccessUnitReader::new(Cursor::new(stream));
        let decoded_first = reader.next_access_unit().unwrap().unwrap();
        let decoded_second = reader.next_access_unit().unwrap().unwrap();
        assert!(decoded_first.ends_with(&first));
        assert_eq!(decoded_second, second);
        assert!(reader.next_access_unit().unwrap().is_none());
    }

    #[test]
    fn annex_b_reader_rejects_delta_only_output() {
        let delta = vec![0, 0, 0, 1, 9, 0xf0, 0, 0, 0, 1, 1, 1];
        let mut reader = AnnexBAccessUnitReader::new(Cursor::new(delta));
        assert_eq!(
            reader.next_access_unit().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
