use crate::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(u32);

impl SessionId {
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let formatted = format!("sd-{:03}", self.0);
        f.pad(&formatted)
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({self})")
    }
}

impl FromStr for SessionId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits = s.strip_prefix("sd-").ok_or(Error::Protocol)?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::Protocol);
        }
        let n: u32 = digits.parse().map_err(|_| Error::Protocol)?;
        if n == 0 {
            return Err(Error::Protocol);
        }
        Ok(Self(n))
    }
}

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserName(String);

impl UserName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl From<&str> for UserName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for UserName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<str> for UserName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionId, UserName};

    #[test]
    fn display_honors_alignment_width() {
        assert_eq!(format!("{:<10}", SessionId::new(1)), "sd-001    ");
        assert_eq!(format!("{:<10}", UserName::new("alice")), "alice     ");
    }
}
