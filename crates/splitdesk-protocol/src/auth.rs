use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use splitdesk_core::{Error, SessionId, UserName};
use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const AUTH_TOKEN_HEX_LEN: usize = 64;

#[derive(Clone, Zeroize, ZeroizeOnDrop, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthToken(String);

impl AuthToken {
    pub fn as_hex(&self) -> &str {
        &self.0
    }

    pub fn parse(hex: &str) -> Result<Self, Error> {
        if hex.len() != AUTH_TOKEN_HEX_LEN || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Auth);
        }
        Ok(Self(hex.to_ascii_lowercase()))
    }
}

impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthToken(<redacted>)")
    }
}

impl fmt::Display for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted-token>")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBinding {
    pub user: UserName,
    pub session_id: SessionId,
    pub exp: u64,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_hex_32() -> String {
    let mut raw = [0u8; 32];
    raw[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    raw[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    let mut out = String::with_capacity(AUTH_TOKEN_HEX_LEN);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for b in raw {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

pub struct TokenStore {
    inner: Mutex<HashMap<String, TokenBinding>>,
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn issue(&self, user: UserName, session_id: SessionId, ttl_secs: u64) -> AuthToken {
        self.issue_at(user, session_id, unix_now(), ttl_secs)
    }

    pub fn issue_at(
        &self,
        user: UserName,
        session_id: SessionId,
        now: u64,
        ttl_secs: u64,
    ) -> AuthToken {
        let hex = random_hex_32();
        let binding = TokenBinding {
            user,
            session_id,
            exp: now.saturating_add(ttl_secs),
        };
        self.inner.lock().insert(hex.clone(), binding);
        AuthToken(hex)
    }

    pub fn verify(
        &self,
        token: &AuthToken,
        user: &UserName,
        session_id: &SessionId,
    ) -> Result<TokenBinding, Error> {
        self.verify_at(token, user, session_id, unix_now())
    }

    pub fn verify_at(
        &self,
        token: &AuthToken,
        user: &UserName,
        session_id: &SessionId,
        now: u64,
    ) -> Result<TokenBinding, Error> {
        let guard = self.inner.lock();
        let binding = guard.get(token.as_hex()).ok_or(Error::Auth)?;
        if now >= binding.exp {
            return Err(Error::Auth);
        }
        if &binding.user != user || &binding.session_id != session_id {
            return Err(Error::Auth);
        }
        Ok(binding.clone())
    }

    pub fn revoke(&self, token: &AuthToken) {
        self.inner.lock().remove(token.as_hex());
    }

    pub fn purge_expired(&self, now: u64) -> usize {
        let mut guard = self.inner.lock();
        let before = guard.len();
        guard.retain(|_, binding| now < binding.exp);
        before - guard.len()
    }
}
