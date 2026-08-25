//! WTS session binding. Compiled only on Windows.
//!
//! Session 0 isolation: services run in session 0, which has no interactive
//! `WinSta0` desktop for a logged-on user. DXGI Desktop Duplication and
//! interactive capture must target a non-zero session (typically
//! `WTSGetActiveConsoleSessionId`). This module never captures from session 0.
//!
//! `WTSQueryUserToken` requires LocalSystem with `SE_TCB_NAME`.
//! `CreateProcessAsUserW` must set `lpDesktop` to `winsta0\\default` and must
//! not inherit handles across sessions.
//!
//! These wrappers bind a process to an **existing** WTS session. They do not
//! create extra interactive logons, patch `termsrv.dll`, or use RDP Wrapper.

use crate::WINDOWS_SESSION_0;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTSEnumerateSessionsExW, WTSFreeMemoryExW, WTSGetActiveConsoleSessionId,
    WTSQueryUserToken, WTSTypeSessionInfoLevel1, WTS_CONNECTSTATE_CLASS, WTS_CURRENT_SERVER_HANDLE,
    WTS_SESSION_INFO_1W,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcessId, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION,
    STARTUPINFOW,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WtsSessionState {
    Active,
    Connected,
    ConnectQuery,
    Shadow,
    Disconnected,
    Idle,
    Listen,
    Reset,
    Down,
    Init,
    Other(i32),
}

impl From<WTS_CONNECTSTATE_CLASS> for WtsSessionState {
    fn from(value: WTS_CONNECTSTATE_CLASS) -> Self {
        match value.0 {
            0 => Self::Active,
            1 => Self::Connected,
            2 => Self::ConnectQuery,
            3 => Self::Shadow,
            4 => Self::Disconnected,
            5 => Self::Idle,
            6 => Self::Listen,
            7 => Self::Reset,
            8 => Self::Down,
            9 => Self::Init,
            other => Self::Other(other),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WtsSession {
    pub session_id: u32,
    pub user_name: String,
    pub domain_name: String,
    pub state: WtsSessionState,
}

pub fn current_process_session_id() -> u32 {
    let mut session = WINDOWS_SESSION_0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }.is_ok() {
        session
    } else {
        WINDOWS_SESSION_0
    }
}

pub fn active_console_session_id() -> Result<u32, String> {
    let id = unsafe { WTSGetActiveConsoleSessionId() };
    if id == u32::MAX {
        Err("WTSGetActiveConsoleSessionId returned 0xFFFFFFFF (no console session)".into())
    } else {
        Ok(id)
    }
}

/// `WTSEnumerateSessionsExW` with level=1, filter=0.
pub fn enumerate_sessions_ex() -> Result<Vec<WtsSession>, String> {
    let mut level = 1u32;
    let mut count = 0u32;
    let mut info: *mut WTS_SESSION_INFO_1W = std::ptr::null_mut();
    unsafe {
        WTSEnumerateSessionsExW(
            WTS_CURRENT_SERVER_HANDLE,
            &mut level,
            0,
            &mut info,
            &mut count,
        )
    }
    .map_err(|e| format!("WTSEnumerateSessionsExW failed: {e}"))?;
    if info.is_null() {
        return Ok(Vec::new());
    }
    let slice = unsafe { std::slice::from_raw_parts(info, count as usize) };
    let mut out = Vec::with_capacity(slice.len());
    for row in slice {
        out.push(WtsSession {
            session_id: row.SessionId,
            user_name: pwstr_to_string(row.pUserName),
            domain_name: pwstr_to_string(row.pDomainName),
            state: WtsSessionState::from(row.State),
        });
    }
    unsafe {
        let _ = WTSFreeMemoryExW(WTSTypeSessionInfoLevel1, info as *mut _, count);
    }
    Ok(out)
}

/// Primary token for `session_id`. Requires LocalSystem + `SE_TCB_NAME`.
///
/// The caller owns the HANDLE and must close it. Session 0 is rejected.
pub fn query_user_token(session_id: u32) -> Result<HANDLE, String> {
    if session_id == WINDOWS_SESSION_0 {
        return Err(
            "WTSQueryUserToken refused for Session 0 (non-interactive service session)".into(),
        );
    }
    let mut token = HANDLE::default();
    unsafe { WTSQueryUserToken(session_id, &mut token) }.map_err(|e| {
        format!("WTSQueryUserToken({session_id}) failed (needs LocalSystem + SE_TCB_NAME): {e}")
    })?;
    Ok(token)
}

pub struct SpawnedProcess {
    pub process_id: u32,
    pub thread_id: u32,
}

/// Spawn `application` in the session that owns `user_token`.
///
/// `lpDesktop` is `winsta0\\default`. Handles are not inherited.
/// Does not create a new Windows logon session.
pub fn create_process_as_user(
    user_token: HANDLE,
    application: &str,
    command_line: Option<&str>,
) -> Result<SpawnedProcess, String> {
    let mut app: Vec<u16> = application
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut cmd = command_line.map(|s| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    });
    let mut desktop: Vec<u16> = "winsta0\\default"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let cmd_ptr = match cmd.as_mut() {
        Some(buf) => PWSTR(buf.as_mut_ptr()),
        None => PWSTR::null(),
    };
    unsafe {
        CreateProcessAsUserW(
            user_token,
            PCWSTR(app.as_mut_ptr()),
            cmd_ptr,
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT,
            None,
            None,
            &si,
            &mut pi,
        )
    }
    .map_err(|e| format!("CreateProcessAsUserW failed: {e}"))?;
    unsafe {
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
    }
    Ok(SpawnedProcess {
        process_id: pi.dwProcessId,
        thread_id: pi.dwThreadId,
    })
}

fn pwstr_to_string(ptr: PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { ptr.to_string().unwrap_or_default() }
}
