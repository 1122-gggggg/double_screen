//! Spawn per-user Weston without taking seat0 DRM master.
//!
//! `-Bpipewire --renderer=gl` is a headless output: it never takes DRM master.
//! A compositor-local SplitDesk module creates the session's only virtual
//! seat. PipeWire and NVENC are required; a session is not reported Running
//! when any mandatory component is missing.

use std::ffi::CString;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use splitdesk_core::{Error, Resolution};

use crate::backend::LinuxSessionRuntime;
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

pub(crate) struct SpawnedCompositor {
    pub handle: CompositorHandle,
    pub runtime: LinuxSessionRuntime,
}

struct SessionPaths {
    runtime_dir: PathBuf,
    session_dir: PathBuf,
    config: PathBuf,
    input_socket: PathBuf,
    module: PathBuf,
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

pub(crate) fn weston_argv(
    socket: &str,
    resolution: Resolution,
    config: &Path,
    module: &Path,
) -> Vec<String> {
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
        "-Bpipewire".to_string(),
        "--renderer=gl".to_string(),
        format!("--socket={socket}"),
        format!("--width={width}"),
        format!("--height={height}"),
        "--idle-time=0".to_string(),
        format!("--modules={}", module.display()),
        format!("--config={}", config.display()),
    ]
}

pub(crate) fn spawn_compositor(
    user: &ResolvedUser,
    socket: &str,
    resolution: Resolution,
    fps: u32,
    memory_limit: Option<&str>,
    cpu_affinity: Option<&str>,
) -> Result<SpawnedCompositor, Error> {
    let weston = which("weston").ok_or_else(|| Error::backend("weston not found on PATH"))?;
    if !weston_backend_present("pipewire") {
        return Err(Error::backend("Weston PipeWire backend is not installed"));
    }
    validate_required_runtime(user)?;
    let paths = prepare_session_paths(user, socket, resolution, fps)?;
    let daemon_root = current_uid() == 0;

    let args = weston_argv(socket, resolution, &paths.config, &paths.module);
    let result = if daemon_root && which("systemd-run").is_some() {
        match spawn_via_systemd(user, &weston, &args, &paths, memory_limit, cpu_affinity) {
            Ok(handle) => Ok(handle),
            Err(err) => {
                tracing::warn!(error = %err, "systemd-run spawn failed; trying setuid weston");
                spawn_direct(user, &weston, &args, &paths)
            }
        }
    } else {
        if !daemon_root && (memory_limit.is_some() || cpu_affinity.is_some()) {
            tracing::warn!(
                "MemoryMax/AllowedCPUs requested but systemd-run is only used when the daemon is root"
            );
        }
        spawn_direct(user, &weston, &args, &paths)
    };
    let mut handle = match result {
        Ok(handle) => handle,
        Err(error) => {
            cleanup_paths(&paths);
            return Err(error);
        }
    };
    let wayland_socket = paths.runtime_dir.join(socket);
    if !confirm_ready(&mut handle, &wayland_socket, &paths.input_socket, user.uid) {
        terminate_handle(handle);
        cleanup_paths(&paths);
        return Err(Error::backend(
            "Weston exited or did not create its Wayland and isolated input sockets",
        ));
    }
    tracing::info!(
        socket,
        "using weston -Bpipewire --renderer=gl (no DRM master)"
    );
    Ok(SpawnedCompositor {
        handle,
        runtime: LinuxSessionRuntime {
            user: user.name.clone(),
            uid: user.uid,
            gid: user.gid,
            home: user.home.clone(),
            runtime_dir: paths.runtime_dir,
            session_dir: paths.session_dir,
            input_socket: paths.input_socket,
            pipewire_target: "weston.pipewire".to_string(),
            resolution,
            fps: fps.max(1),
        },
    })
}

fn validate_required_runtime(user: &ResolvedUser) -> Result<(), Error> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    if which("gst-launch-1.0").is_none() || which("gst-inspect-1.0").is_none() {
        return Err(Error::backend(
            "GStreamer gst-launch-1.0 and gst-inspect-1.0 are required",
        ));
    }
    for element in [
        "pipewiresrc",
        "queue",
        "glupload",
        "glcolorconvert",
        "nvh264enc",
        "h264parse",
        "fdsink",
    ] {
        let available = Command::new("gst-inspect-1.0")
            .arg(element)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !available {
            return Err(Error::backend(format!(
                "required GStreamer element {element} is unavailable"
            )));
        }
    }
    let runtime = ensure_runtime_dir(user)?;
    let pipewire_socket = runtime.join("pipewire-0");
    let metadata = std::fs::metadata(&pipewire_socket).map_err(|error| {
        Error::backend(format!(
            "user PipeWire socket {} is unavailable: {error}; start pipewire.service for {}",
            pipewire_socket.display(),
            user.name
        ))
    })?;
    if !metadata.file_type().is_socket() || metadata.uid() != user.uid {
        return Err(Error::IsolationViolation);
    }
    Ok(())
}

fn input_module_path() -> Result<PathBuf, Error> {
    use std::os::unix::fs::MetadataExt;

    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("SPLITDESK_WESTON_INPUT_MODULE") {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend([
        PathBuf::from("/usr/local/lib/splitdesk/splitdesk-input.so"),
        PathBuf::from("/usr/lib/splitdesk/splitdesk-input.so"),
        PathBuf::from("/usr/lib64/splitdesk/splitdesk-input.so"),
    ]);
    for candidate in candidates {
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if candidate.is_absolute() && metadata.is_file() && metadata.mode() & 0o022 == 0 {
            return Ok(candidate);
        }
    }
    Err(Error::backend(
        "splitdesk-input.so is missing or writable by group/other; run packaging/linux/install.sh",
    ))
}

fn prepare_session_paths(
    user: &ResolvedUser,
    socket: &str,
    resolution: Resolution,
    fps: u32,
) -> Result<SessionPaths, Error> {
    use std::os::unix::fs::OpenOptionsExt;

    let module = input_module_path()?;
    let runtime_dir = ensure_runtime_dir(user)?;
    let splitdesk_dir = runtime_dir.join("splitdesk");
    let session_dir = splitdesk_dir.join(socket);
    ensure_private_directory(&splitdesk_dir, user, false)?;
    ensure_private_directory(&session_dir, user, true)?;
    let input_socket = session_dir.join("input.sock");

    let config = session_dir.join("weston.ini");
    let width = resolution.width.max(1);
    let height = resolution.height.max(1);
    let contents = format!(
        "[core]\nrequire-input=false\n\n[output]\nname=pipewire\nmode={width}x{height}@{}\n",
        fps.max(1)
    );
    let config_result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&config)
            .map_err(|error| Error::backend(format!("open {}: {error}", config.display())))?;
        file.write_all(contents.as_bytes())
            .map_err(|error| Error::backend(format!("write {}: {error}", config.display())))?;
        apply_file_ownership(&config, user, 0o600)
    })();
    if let Err(error) = config_result {
        let _ = std::fs::remove_file(&config);
        let _ = std::fs::remove_dir(&session_dir);
        return Err(error);
    }

    Ok(SessionPaths {
        runtime_dir,
        session_dir,
        config,
        input_socket,
        module,
    })
}

fn cleanup_paths(paths: &SessionPaths) {
    let _ = std::fs::remove_file(&paths.input_socket);
    let _ = std::fs::remove_file(&paths.config);
    let _ = std::fs::remove_dir(&paths.session_dir);
}

pub(crate) fn cleanup_runtime(runtime: &LinuxSessionRuntime) {
    let _ = std::fs::remove_file(&runtime.input_socket);
    let _ = std::fs::remove_file(runtime.session_dir.join("weston.ini"));
    let _ = std::fs::remove_dir(&runtime.session_dir);
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

fn confirm_ready(handle: &mut CompositorHandle, wayland: &Path, input: &Path, uid: u32) -> bool {
    for attempt in 0..100 {
        std::thread::sleep(Duration::from_millis(50));
        match handle {
            CompositorHandle::Child(child) => match child.try_wait() {
                Ok(Some(_)) => return false,
                Ok(None) => {}
                Err(_) => return false,
            },
            CompositorHandle::Systemd { unit } => match systemd_active_state(unit).as_deref() {
                Some("failed") => return false,
                Some("inactive") | Some("dead") if attempt >= 4 => return false,
                Some("active") | Some("activating") => {}
                _ => {}
            },
        }
        if socket_ready(wayland, uid, false) && socket_ready(input, uid, true) {
            return true;
        }
    }
    false
}

fn socket_ready(path: &Path, uid: u32, owner_only: bool) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_socket()
        && metadata.uid() == uid
        && (!owner_only || metadata.mode() & 0o077 == 0)
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
    paths: &SessionPaths,
    memory_limit: Option<&str>,
    cpu_affinity: Option<&str>,
) -> Result<CompositorHandle, Error> {
    let socket = args
        .iter()
        .find_map(|a| a.strip_prefix("--socket="))
        .unwrap_or("splitdesk-0");
    let unit = format!("splitdesk-sess-{socket}-pipewire");
    let mut cmd = Command::new("systemd-run");
    cmd.arg(format!("--uid={}", user.name));
    cmd.arg(format!("--gid={}", user.gid));
    cmd.arg("--same-dir");
    cmd.arg("--no-block");
    cmd.arg(format!("--unit={unit}"));
    cmd.arg(format!("--working-directory={}", user.home.display()));
    cmd.arg(format!(
        "--setenv=XDG_RUNTIME_DIR={}",
        paths.runtime_dir.display()
    ));
    cmd.arg("--setenv=XDG_SESSION_TYPE=wayland");
    cmd.arg(format!("--setenv=HOME={}", user.home.display()));
    cmd.arg(format!("--setenv=USER={}", user.name));
    cmd.arg(format!("--setenv=LOGNAME={}", user.name));
    cmd.arg(format!(
        "--setenv=SPLITDESK_INPUT_SOCKET={}",
        paths.input_socket.display()
    ));
    cmd.arg("--setenv=PIPEWIRE_REMOTE=pipewire-0");
    cmd.arg(format!(
        "--setenv=DBUS_SESSION_BUS_ADDRESS=unix:path={}/bus",
        paths.runtime_dir.display()
    ));
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
    paths: &SessionPaths,
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
    cmd.env("XDG_RUNTIME_DIR", &paths.runtime_dir);
    cmd.env("XDG_SESSION_TYPE", "wayland");
    cmd.env("SPLITDESK_INPUT_SOCKET", &paths.input_socket);
    cmd.env("PIPEWIRE_REMOTE", "pipewire-0");
    cmd.env(
        "DBUS_SESSION_BUS_ADDRESS",
        format!("unix:path={}/bus", paths.runtime_dir.display()),
    );
    cmd.current_dir(&user.home);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::inherit());
    if current_uid() == 0 {
        let cname = CString::new(user.name.as_str()).map_err(|_| Error::UserNotFound)?;
        let uid = user.uid;
        let gid = user.gid;
        unsafe {
            cmd.pre_exec(move || {
                if libc::initgroups(cname.as_ptr(), gid) != 0
                    || libc::setgid(gid) != 0
                    || libc::setuid(uid) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
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
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| Error::BackendUnavailable {
        detail: format!("stat {}: {e}", dir.display()),
    })?;
    use std::os::unix::fs::MetadataExt;
    if !meta.file_type().is_dir() || meta.uid() != user.uid || meta.mode() & 0o077 != 0 {
        return Err(Error::BackendUnavailable {
            detail: format!(
                "XDG_RUNTIME_DIR {} must be a real owner-only directory owned by uid {}",
                dir.display(),
                user.uid
            ),
        });
    }
    Ok(dir)
}

fn ensure_private_directory(
    dir: &Path,
    user: &ResolvedUser,
    must_be_new: bool,
) -> Result<(), Error> {
    use std::os::unix::fs::MetadataExt;

    match std::fs::create_dir(dir) {
        Ok(()) => {
            if let Err(error) = apply_runtime_ownership(dir, user) {
                let _ = std::fs::remove_dir(dir);
                return Err(error);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && !must_be_new => {
            let metadata = std::fs::symlink_metadata(dir)
                .map_err(|error| Error::backend(format!("stat {}: {error}", dir.display())))?;
            if !metadata.file_type().is_dir()
                || metadata.uid() != user.uid
                || metadata.mode() & 0o077 != 0
            {
                return Err(Error::IsolationViolation);
            }
            Ok(())
        }
        Err(error) => Err(Error::backend(format!(
            "create private session directory {}: {error}",
            dir.display()
        ))),
    }
}

fn apply_runtime_ownership(dir: &Path, user: &ResolvedUser) -> Result<(), Error> {
    let cpath =
        CString::new(dir.to_string_lossy().as_bytes()).map_err(|_| Error::BackendUnavailable {
            detail: format!("invalid runtime dir {}", dir.display()),
        })?;
    unsafe {
        if current_uid() == 0 && libc::chown(cpath.as_ptr(), user.uid, user.gid) != 0 {
            return Err(Error::BackendUnavailable {
                detail: format!(
                    "chown {} failed: {}",
                    dir.display(),
                    std::io::Error::last_os_error()
                ),
            });
        }
        if current_uid() != 0 && current_uid() != user.uid {
            return Err(Error::backend(format!(
                "cannot assign {} to uid {} from uid {}",
                dir.display(),
                user.uid,
                current_uid()
            )));
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

fn apply_file_ownership(path: &Path, user: &ResolvedUser, mode: libc::mode_t) -> Result<(), Error> {
    let cpath =
        CString::new(path.to_string_lossy().as_bytes()).map_err(|_| Error::BackendUnavailable {
            detail: format!("invalid path {}", path.display()),
        })?;
    unsafe {
        if current_uid() == 0 && libc::chown(cpath.as_ptr(), user.uid, user.gid) != 0 {
            return Err(Error::backend(format!(
                "chown {} failed: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }
        if libc::chmod(cpath.as_ptr(), mode) != 0 {
            return Err(Error::backend(format!(
                "chmod {:o} {} failed: {}",
                mode,
                path.display(),
                std::io::Error::last_os_error()
            )));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weston_command_is_pipewire_only_and_loads_private_input_module() {
        let config = Path::new("/run/user/1000/splitdesk/splitdesk-1/weston.ini");
        let module = Path::new("/usr/lib/splitdesk/splitdesk-input.so");
        let argv = weston_argv("splitdesk-1", Resolution::new(1280, 720), config, module);

        assert!(argv.iter().any(|arg| arg == "-Bpipewire"));
        assert!(argv.iter().any(|arg| arg == "--renderer=gl"));
        assert!(argv.iter().any(|arg| arg == "--socket=splitdesk-1"));
        assert!(argv
            .iter()
            .any(|arg| arg == "--modules=/usr/lib/splitdesk/splitdesk-input.so"));
        assert!(argv
            .iter()
            .any(|arg| { arg == "--config=/run/user/1000/splitdesk/splitdesk-1/weston.ini" }));
        assert!(!argv.iter().any(|arg| arg.contains("drm")));
        assert!(!argv.iter().any(|arg| arg.contains("headless")));
    }
}
