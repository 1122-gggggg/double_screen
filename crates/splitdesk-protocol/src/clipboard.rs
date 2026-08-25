use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardUtf8 {
    pub text: String,
}

impl ClipboardUtf8 {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl fmt::Debug for ClipboardUtf8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClipboardUtf8")
            .field("len", &self.text.len())
            .field("text", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for ClipboardUtf8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClipboardUtf8(len={})", self.text.len())
    }
}
