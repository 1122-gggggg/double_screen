use splitdesk_core::Error;
use splitdesk_protocol::Input;
use std::collections::HashSet;

pub struct WindowsInputBackend {
    keys_down: HashSet<u16>,
    buttons_down: HashSet<u32>,
}

impl Default for WindowsInputBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsInputBackend {
    pub fn new() -> Self {
        Self {
            keys_down: HashSet::new(),
            buttons_down: HashSet::new(),
        }
    }

    pub fn inject(&mut self, input: &Input) -> Result<(), Error> {
        #[cfg(not(windows))]
        {
            let _ = input;
            Err(unavailable())
        }
        #[cfg(windows)]
        {
            match input {
                Input::PointerMotion { x, y, .. } => send_motion(*x, *y),
                Input::PointerButton {
                    button, pressed, ..
                } => {
                    send_button(*button, *pressed)?;
                    if *pressed {
                        self.buttons_down.insert(*button);
                    } else {
                        self.buttons_down.remove(button);
                    }
                    Ok(())
                }
                Input::Key {
                    keycode, pressed, ..
                } => {
                    let keycode = u16::try_from(*keycode).map_err(|_| Error::Protocol)?;
                    send_key(keycode, *pressed)?;
                    if *pressed {
                        self.keys_down.insert(keycode);
                    } else {
                        self.keys_down.remove(&keycode);
                    }
                    Ok(())
                }
                Input::Scroll { dx, dy, .. } => send_scroll(*dx, *dy),
            }
        }
    }

    pub fn release_all(&mut self) -> Result<(), Error> {
        #[cfg(not(windows))]
        {
            Err(unavailable())
        }
        #[cfg(windows)]
        {
            let mut first_error = None;
            let keys: Vec<u16> = self.keys_down.iter().copied().collect();
            for keycode in keys {
                match send_key(keycode, false) {
                    Ok(()) => {
                        self.keys_down.remove(&keycode);
                    }
                    Err(err) if first_error.is_none() => first_error = Some(err),
                    Err(_) => {}
                }
            }

            let buttons: Vec<u32> = self.buttons_down.iter().copied().collect();
            for button in buttons {
                match send_button(button, false) {
                    Ok(()) => {
                        self.buttons_down.remove(&button);
                    }
                    Err(err) if first_error.is_none() => first_error = Some(err),
                    Err(_) => {}
                }
            }

            match first_error {
                Some(err) => Err(err),
                None => Ok(()),
            }
        }
    }
}

#[cfg(not(windows))]
fn unavailable() -> Error {
    Error::BackendUnavailable {
        detail: "Windows SendInput is not available on this target".into(),
    }
}

#[cfg(windows)]
fn send_motion(x: f64, y: f64) -> Result<(), Error> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    if !x.is_finite() || !y.is_finite() {
        return Err(Error::Protocol);
    }

    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if width <= 0 || height <= 0 {
        return Err(Error::BackendUnavailable {
            detail: "GetSystemMetrics returned an empty virtual desktop".into(),
        });
    }

    let normalized_x = normalize_absolute(x, left, width);
    let normalized_y = normalize_absolute(y, top, height);
    send_mouse(
        normalized_x,
        normalized_y,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

#[cfg(windows)]
fn normalize_absolute(position: f64, origin: i32, extent: i32) -> i32 {
    if extent <= 1 {
        return 0;
    }
    (((position - f64::from(origin)) * 65535.0 / f64::from(extent - 1)).round()).clamp(0.0, 65535.0)
        as i32
}

#[cfg(windows)]
fn send_button(button: u32, pressed: bool) -> Result<(), Error> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2};

    let (flags, data) = match (button, pressed) {
        (1, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (1, false) => (MOUSEEVENTF_LEFTUP, 0),
        (2, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (2, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (3, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (3, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (4, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON1)),
        (4, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON1)),
        (5, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON2)),
        (5, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON2)),
        _ => return Err(Error::Protocol),
    };
    send_mouse(0, 0, data, flags)
}

#[cfg(windows)]
fn send_scroll(dx: f64, dy: f64) -> Result<(), Error> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL};

    if !dx.is_finite() || !dy.is_finite() {
        return Err(Error::Protocol);
    }
    let horizontal = wheel_amount(dx);
    let vertical = wheel_amount(dy);
    if horizontal != 0 {
        send_mouse(0, 0, horizontal, MOUSEEVENTF_HWHEEL)?;
    }
    if vertical != 0 {
        send_mouse(0, 0, vertical, MOUSEEVENTF_WHEEL)?;
    }
    Ok(())
}

#[cfg(windows)]
fn wheel_amount(delta: f64) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::WHEEL_DELTA;

    (delta * f64::from(WHEEL_DELTA))
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32 as u32
}

#[cfg(windows)]
fn send_key(keycode: u16, pressed: bool) -> Result<(), Error> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(keycode),
                dwFlags: if pressed {
                    Default::default()
                } else {
                    KEYEVENTF_KEYUP
                },
                ..Default::default()
            },
        },
    };
    send_inputs(&[input])
}

#[cfg(windows)]
fn send_mouse(
    dx: i32,
    dy: i32,
    mouse_data: u32,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> Result<(), Error> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT};

    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: mouse_data,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    send_inputs(&[input])
}

#[cfg(windows)]
fn send_inputs(inputs: &[windows::Win32::UI::Input::KeyboardAndMouse::INPUT]) -> Result<(), Error> {
    use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;

    let inserted = unsafe { SendInput(inputs, std::mem::size_of_val(&inputs[0]) as i32) };
    if inserted == inputs.len() as u32 {
        Ok(())
    } else {
        Err(Error::BackendUnavailable {
            detail: format!(
                "SendInput inserted {inserted}/{} events: {}",
                inputs.len(),
                windows::core::Error::from_win32()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_all_with_no_injected_state_does_not_panic() {
        let mut backend = WindowsInputBackend::new();
        let result = backend.release_all();
        if cfg!(windows) {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(Error::BackendUnavailable { .. })));
        }
    }
}
