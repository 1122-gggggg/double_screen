use std::io::{self, Write};
use std::path::{Path, PathBuf};

use zeroize::{Zeroize, ZeroizeOnDrop};

pub const TOKEN_BYTE_LEN: usize = 32;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Token(String);

impl Token {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

pub fn generate_token() -> Token {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut bytes = [0u8; TOKEN_BYTE_LEN];
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    Token(hex_encode(&bytes))
}

pub fn default_token_path() -> PathBuf {
    #[cfg(unix)]
    {
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            if !runtime.is_empty() {
                return PathBuf::from(runtime)
                    .join("splitdesk")
                    .join("daemon.token");
            }
        }
        PathBuf::from(format!("/tmp/splitdesk-{}/daemon.token", unix_uid()))
    }
    #[cfg(windows)]
    {
        let local = std::env::var("LOCALAPPDATA")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        PathBuf::from(local).join("SplitDesk").join("daemon.token")
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::env::temp_dir().join("splitdesk").join("daemon.token")
    }
}

#[cfg(unix)]
fn unix_uid() -> u32 {
    unsafe { libc::getuid() }
}

pub fn write_token_file(path: &Path, token: &Token) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(token.as_str().as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(token.as_str().as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
    }

    Ok(())
}

pub fn load_token(path: Option<&Path>) -> io::Result<Token> {
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_token_path);
    let raw = std::fs::read_to_string(&path)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "token file empty",
        ));
    }
    Ok(Token::new(trimmed.to_string()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
