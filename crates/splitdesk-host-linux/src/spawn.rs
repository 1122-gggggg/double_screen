//! Spawn per-user Weston without taking seat0 DRM master.
//!
//! Phase 0: `-Bpipewire --renderer=gl` (output-only PipeWire node). AUTO
//! renderer is Pixman, so `--renderer=gl` is always passed. If the PipeWire
//! backend is missing, fall back to `-Bheadless --renderer=gl` for
//! EGLDevice / `EGL_PLATFORM_SURFACELESS_MESA`. Never pass `-Bdrm`. GPU
//! access is via `/dev/dri/renderD*` only.

use std::ffi::CString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use splitdesk_core::{Error, Resolution};

use crate::detect::{run_capture, which};

pub(crate) struct ResolvedUser {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

pub(crate) enum CompositorHandle {
    Child(Child),
    Systemd { unit: String },
}

pub(crate) fn resolve_user(name: &str) -> Result<ResolvedUser, Error> {
    let cname = CString::new(name).map_err(|_| Error::UserNotFound)?;
    unsafe {
        let pwd = libc::getpwnam(cname.as_ptr());
        if pwd.is_null() {
            return Err(Error::UserNotFound);
        }
        let uid = (*pwd).pw_uid;
        let gid = (*pwd).pw_gid;
        if (*pwd).pw_dir.is_null() {
            return Err(Error::UserNotFound);
        }
        let home = std::ffi::CStr::from_ptr((*pwd).pw_dir)
            .to_string_lossy()
            .into_owned();
        Ok(ResolvedUser {
            name: name.to_string(),
            uid,
            gid,
            home: PathBuf::from(home),
        })
    }
}

pub(crate) fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

pub(crate) fn authorize_user(user: &ResolvedUser) -> Result<(), Error> {
    if user.uid == 0 {
        return Err(Error::PermissionDenied);
    }
    let me = current_uid();
    if me != 0 && me != user.uid {
        return Err(Error::PermissionDenied);
    }
    Ok(())
}

pub(crate) fn weston_argv(socket: &str, resolution: Resolution, backend: &str) -> Vec<String> {
    let width = if resolution.width == 0 {
        1920
    } else {
        resolution.width
    };
    let height = if resolution.height == 0 {
        1080
    } else {
        resolution.height
    };
    vec![
        format!("-B{backend}"),
        "--renderer=gl".to_string(),
        format!("--socket={socket}"),
        format!("--width={width}"),
        format!("--height={height}"),
        "--idle-time=0".to_string(),
        "--no-config".to_string(),
    ]
}

pub(crate) fn spawn_compositor(
    user: &ResolvedUser,
    socket: &str,
    resolution: Resolution,
    memory_limit: Option<&str>,
    cpu_affinity: Option<&str>,
) -> Result<CompositorHandle, Error> {
    let weston = which("weston").ok_or_else(|| Error::backend("weston not found on PATH"))?;
    let runtime = ensure_runtime_dir(user)?;
    let daemon_root = current_uid() == 0;
    let mut backends = Vec::new();
    if weston_backend_present("pipewire") {
        backends.push("pipewire");
    }
    if weston_backend_present("headless") {
        backends.push("headless");
    }
    if backends.is_empty() {
        backends.extend(["pipewire", "headless"]);
    }

    let mut last_err = Error::backend("no weston backend started");
    for backend in backends {
        let args = weston_argv(socket, resolution, backend);
        let result = if daemon_root && which("systemd-run").is_some() {
            match spawn_via_systemd(user, &weston, &args, &runtime, memory_limit, cpu_affinity) {
                Ok(handle) => Ok(handle),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        backend,
                        "systemd-run spawn failed; trying setuid weston"
                    );
                    spawn_direct(user, &weston, &args, &runtime)
                }
            }
        } else {
            if !daemon_root && (memory_limit.is_some() || cpu_affinity.is_some()) {
                tracing::warn!(
                    "MemoryMax/AllowedCPUs requested but systemd-run is only used when the daemon is root"
                );
            }
            spawn_direct(user, &weston, &args, &runtime)
        };
        match result {
            Ok(mut handle) => {
                if confirm_alive(&mut handle) {
                    if backend == "headless" {
                        tracing::info!(
                            socket,
                            "using weston -Bheadless --renderer=gl (EGL surfaceless, no DRM master)"
                        );
                    } else {
                        tracing::info!(
                            socket,
                            "using weston -Bpipewire --renderer=gl (no DRM master)"
                        );
                    }
                    return Ok(handle);
                }
                tracing::warn!(backend, "weston exited immediately; trying next backend");
                last_err = Error::backend(format!("weston -B{backend} exited immediately"));
                terminate_handle(handle);
            }
            Err(err) => {
                tracing::warn!(backend, error = %err, "weston backend failed");
                last_err = err;
            }
        }
    }
    Err(last_err)
}

fn weston_backend_present(name: &str) -> bool {
    if let Some(help) = run_capture("weston", &["--help"]) {
        let needle_flag = format!("-B{name}");
        let needle_so = format!("{name}-backend");
        if help.contains(&needle_flag) || help.contains(&needle_so) {
            return true;
        }
    }
    let so = format!("{name}-backend.so");
    const DIRS: &[&str] = &[
        "/usr/lib/weston",
        "/usr/lib64/weston",
        "/usr/lib/x86_64-linux-gnu/weston",
        "/usr/lib/aarch64-linux-gnu/weston",
        "/usr/local/lib/weston",
    ];
    for dir in DIRS {
        if Path::new(dir).join(&so).is_file() {
            return true;
        }
    }
    for n in 9..18 {
        let a = format!("/usr/lib/libweston-{n}/{so}");
        let b = format!("/usr/lib/x86_64-linux-gnu/libweston-{n}/{so}");
        let c = format!("/usr/lib64/libweston-{n}/{so}");
        if Path::new(&a).is_file() || Path::new(&b).is_file() || Path::new(&c).is_file() {
            return true;
        }
    }
    false
}

fn confirm_alive(handle: &mut CompositorHandle) -> bool {
    for _ in 0..8 {
        std::thread::sleep(Duration::from_millis(50));
        match handle {
            CompositorHandle::Child(child) => match child.try_wait() {
                Ok(Some(_)) => return false,
                Ok(None) => {}
                Err(_) => return false,
            },
            CompositorHandle::Systemd { unit } => match systemd_active_state(unit).as_deref() {
                Some("failed") | Some("inactive") | Some("dead") => return false,
                Some("active") | Some("activating") => return true,
                _ => {}
            },
        }
    }
    match handle {
        CompositorHandle::Child(child) => matches!(child.try_wait(), Ok(None)),
        CompositorHandle::Systemd { unit } => !matches!(
            systemd_active_state(unit).as_deref(),
            Some("failed") | Some("inactive") | Some("dead")
        ),
    }
}

fn systemd_active_state(unit: &str) -> Option<String> {
    let output = Command::new("systemctl")
        .args(["show", "-p", "ActiveState", "--value", unit])
        .output()
        .ok()?;
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if state.is_empty() {
        None
    } else {
        Some(state)
    }
}

fn spawn_via_systemd(
    user: &ResolvedUser,
    weston: &Path,
    args: &[String],
    runtime: &Path,
    memory_limit: Option<&str>,
    cpu_affinity: Option<&str>,
) -> Result<CompositorHandle, Error> {
    let socket = args
        .iter()
        .find_map(|a| a.strip_prefix("--socket="))
        .unwrap_or("splitdesk-0");
    let backend = args
        .iter()
        .find_map(|a| a.strip_prefix("-B"))
        .unwrap_or("headless");
    let unit = format!("splitdesk-sess-{socket}-{backend}");
    let mut cmd = Command::new("systemd-run");
    cmd.arg(format!("--uid={}", user.uid));
    cmd.arg(format!("--gid={}", user.gid));
    cmd.arg("--same-dir");
    cmd.arg("--no-block");
    cmd.arg(format!("--unit={unit}"));
    cmd.arg(format!("--working-directory={}", user.home.display()));
    cmd.arg(format!("--setenv=XDG_RUNTIME_DIR={}", runtime.display()));
    cmd.arg("--setenv=XDG_SESSION_TYPE=wayland");
    cmd.arg(format!("--setenv=HOME={}", user.home.display()));
    cmd.arg("--setenv=WAYLAND_DISPLAY=");
    cmd.arg("--setenv=DISPLAY=");
    if let Some(limit) = memory_limit {
        cmd.arg(format!("--property=MemoryMax={limit}"));
    }
    if let Some(cpus) = cpu_affinity {
        cmd.arg(format!("--property=AllowedCPUs={cpus}"));
    }
    cmd.arg("--collect");
    cmd.arg("--");
    cmd.arg(weston);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let output = cmd.output().map_err(|e| Error::BackendUnavailable {
        detail: format!("systemd-run failed to exec: {e}"),
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::BackendUnavailable {
            detail: format!("systemd-run exited {}: {}", output.status, stderr.trim()),
        });
    }
    Ok(CompositorHandle::Systemd { unit })
}

fn spawn_direct(
    user: &ResolvedUser,
    weston: &Path,
    args: &[String],
    runtime: &Path,
) -> Result<CompositorHandle, Error> {
    let mut cmd = Command::new(weston);
    cmd.args(args);
    cmd.env_clear();
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
    );
    cmd.env("HOME", &user.home);
    cmd.env("USER", &user.name);
    cmd.env("LOGNAME", &user.name);
    cmd.env("XDG_RUNTIME_DIR", runtime);
    cmd.env("XDG_SESSION_TYPE", "wayland");
    cmd.current_dir(&user.home);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    if current_uid() == 0 {
        cmd.uid(user.uid);
        cmd.gid(user.gid);
    }
    let child = cmd.spawn().map_err(|e| Error::BackendUnavailable {
        detail: format!("failed to spawn weston as uid {}: {e}", user.uid),
    })?;
    if child.id() == 0 {
        return Err(Error::BackendUnavailable {
            detail: "weston spawned with pid 0".to_string(),
        });
    }
    Ok(CompositorHandle::Child(child))
}

fn ensure_runtime_dir(user: &ResolvedUser) -> Result<PathBuf, Error> {
    let dir = PathBuf::from(format!("/run/user/{}", user.uid));
    if dir.is_dir() {
        enforce_runtime_mode(&dir, user)?;
        return Ok(dir);
    }
    if current_uid() == 0 {
        std::fs::create_dir_all(&dir).map_err(|e| Error::BackendUnavailable {
            detail: format!("cannot create {}: {e}", dir.display()),
        })?;
        apply_runtime_ownership(&dir, user)?;
        return Ok(dir);
    }
    Err(Error::BackendUnavailable {
        detail: format!("XDG_RUNTIME_DIR {} is missing", dir.display()),
    })
}

fn enforce_runtime_mode(dir: &Path, user: &ResolvedUser) -> Result<(), Error> {
    let meta = std::fs::metadata(dir).map_err(|e| Error::BackendUnavailable {
        detail: format!("stat {}: {e}", dir.display()),
    })?;
    use std::os::unix::fs::MetadataExt;
    if meta.uid() != user.uid {
        if current_uid() == 0 {
            apply_runtime_ownership(dir, user)?;
        } else {
            return Err(Error::BackendUnavailable {
                detail: format!(
                    "XDG_RUNTIME_DIR {} is not owned by uid {}",
                    dir.display(),
                    user.uid
                ),
            });
        }
    }
    if meta.mode() & 0o777 != 0o700 && current_uid() == 0 {
        apply_runtime_ownership(dir, user)?;
    }
    Ok(())
}

fn apply_runtime_ownership(dir: &Path, user: &ResolvedUser) -> Result<(), Error> {
    let cpath =
        CString::new(dir.to_string_lossy().as_bytes()).map_err(|_| Error::BackendUnavailable {
            detail: format!("invalid runtime dir {}", dir.display()),
        })?;
    unsafe {
        if libc::chown(cpath.as_ptr(), user.uid, user.gid) != 0 {
            return Err(Error::BackendUnavailable {
                detail: format!(
                    "chown {} failed: {}",
                    dir.display(),
                    std::io::Error::last_os_error()
                ),
            });
        }
        if libc::chmod(cpath.as_ptr(), 0o700) != 0 {
            return Err(Error::BackendUnavailable {
                detail: format!(
                    "chmod 0700 {} failed: {}",
                    dir.display(),
                    std::io::Error::last_os_error()
                ),
            });
        }
    }
    Ok(())
}

pub(crate) fn terminate_handle(handle: CompositorHandle) {
    match handle {
        CompositorHandle::Child(mut child) => {
            let pid = child.id() as libc::pid_t;
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    _ => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        CompositorHandle::Systemd { unit } => {
            let _ = Command::new("systemctl")
                .args(["stop", &unit])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

pub(crate) fn compositor_pid(handle: &CompositorHandle) -> Option<u32> {
    match handle {
        CompositorHandle::Child(child) => Some(child.id()),
        CompositorHandle::Systemd { unit } => systemd_main_pid(unit),
    }
}

fn systemd_main_pid(unit: &str) -> Option<u32> {
    let output = Command::new("systemctl")
        .args(["show", "-p", "MainPID", "--value", unit])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let pid: u32 = text.trim().parse().ok()?;
    if pid == 0 {
        None
    } else {
        Some(pid)
    }
}
