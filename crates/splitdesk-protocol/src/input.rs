use serde::{Deserialize, Serialize};
use splitdesk_core::Error;
use std::collections::{HashSet, VecDeque};

const RELIABLE_CAP: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum Input {
    PointerMotion {
        x: f64,
        y: f64,
        ts: u64,
    },
    PointerButton {
        button: u32,
        pressed: bool,
        ts: u64,
    },
    Key {
        keycode: u32,
        pressed: bool,
        modifiers: u32,
        ts: u64,
    },
    Scroll {
        dx: f64,
        dy: f64,
        ts: u64,
    },
}

pub struct InputChannels {
    motion: Option<Input>,
    scroll: Option<Input>,
    reliable: VecDeque<Input>,
    reliable_cap: usize,
    keys_down: HashSet<u32>,
    buttons_down: HashSet<u32>,
}

impl Default for InputChannels {
    fn default() -> Self {
        Self::new()
    }
}

impl InputChannels {
    pub fn new() -> Self {
        Self::with_reliable_cap(RELIABLE_CAP)
    }

    pub fn with_reliable_cap(reliable_cap: usize) -> Self {
        Self {
            motion: None,
            scroll: None,
            reliable: VecDeque::new(),
            reliable_cap: reliable_cap.max(1),
            keys_down: HashSet::new(),
            buttons_down: HashSet::new(),
        }
    }

    pub fn push(&mut self, input: Input) -> Result<(), Error> {
        match &input {
            Input::PointerMotion { .. } => {
                self.motion = Some(input);
                Ok(())
            }
            Input::Scroll { .. } => {
                self.scroll = Some(input);
                Ok(())
            }
            Input::PointerButton {
                button, pressed, ..
            } => {
                if *pressed {
                    self.buttons_down.insert(*button);
                } else {
                    self.buttons_down.remove(button);
                }
                self.push_reliable(input)
            }
            Input::Key {
                keycode, pressed, ..
            } => {
                if *pressed {
                    self.keys_down.insert(*keycode);
                } else {
                    self.keys_down.remove(keycode);
                }
                self.push_reliable(input)
            }
        }
    }

    fn push_reliable(&mut self, input: Input) -> Result<(), Error> {
        if self.reliable.len() >= self.reliable_cap {
            return Err(Error::Protocol);
        }
        self.reliable.push_back(input);
        Ok(())
    }

    pub fn take_motion(&mut self) -> Option<Input> {
        self.motion.take()
    }

    pub fn take_scroll(&mut self) -> Option<Input> {
        self.scroll.take()
    }

    pub fn pop_reliable(&mut self) -> Option<Input> {
        self.reliable.pop_front()
    }

    pub fn held_keys(&self) -> impl Iterator<Item = u32> + '_ {
        self.keys_down.iter().copied()
    }

    pub fn held_buttons(&self) -> impl Iterator<Item = u32> + '_ {
        self.buttons_down.iter().copied()
    }

    pub fn release_held(&mut self, ts: u64) -> Vec<Input> {
        let mut out = Vec::with_capacity(self.keys_down.len() + self.buttons_down.len());
        let keys: Vec<u32> = self.keys_down.drain().collect();
        let buttons: Vec<u32> = self.buttons_down.drain().collect();
        for keycode in keys {
            out.push(Input::Key {
                keycode,
                pressed: false,
                modifiers: 0,
                ts,
            });
        }
        for button in buttons {
            out.push(Input::PointerButton {
                button,
                pressed: false,
                ts,
            });
        }
        self.motion = None;
        self.scroll = None;
        out
    }
}
