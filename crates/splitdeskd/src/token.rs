use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        let existed = match std::fs::symlink_metadata(dir) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        if !existed {
            std::fs::create_dir_all(dir)?;
        }
        let metadata = std::fs::symlink_metadata(dir)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "token directory must be a real directory",
            ));
        }
        #[cfg(unix)]
        if !existed {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }

    // Never open the destination for truncate: it may be a symlink, and a
    // reader must observe either the old complete token or the new one.
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "token path is a symlink",
            ));
        }
    }
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("token"),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(token.as_str().as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
        #[cfg(unix)]
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        drop(file);
        replace_token_file(&tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(not(windows))]
fn replace_token_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_token_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    // MOVEFILE_REPLACE_EXISTING provides replacement semantics and
    // MOVEFILE_WRITE_THROUGH waits for the move to reach disk.
    // Source: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(io::Error::other)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "splitdesk-token-test-{}-{}",
            uuid::Uuid::new_v4(),
            name
        ))
    }

    #[test]
    fn roundtrip_and_atomic_replacement() {
        let path = temp_path("token");
        write_token_file(&path, &Token::new("first".into())).unwrap();
        write_token_file(&path, &Token::new("second".into())).unwrap();
        assert_eq!(load_token(Some(&path)).unwrap().as_str(), "second");
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_target_and_keeps_target_unchanged() {
        use std::os::unix::fs::symlink;
        let target = temp_path("target");
        let link = temp_path("link");
        fs::write(&target, "original\n").unwrap();
        symlink(&target, &link).unwrap();
        assert!(write_token_file(&link, &Token::new("replacement".into())).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "original\n");
        let _ = fs::remove_file(link);
        let _ = fs::remove_file(target);
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("mode");
        write_token_file(&path, &Token::new("secret".into())).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_token_directory() {
        use std::os::unix::fs::symlink;
        let target_dir = temp_path("target-dir");
        let linked_dir = temp_path("linked-dir");
        fs::create_dir(&target_dir).unwrap();
        symlink(&target_dir, &linked_dir).unwrap();

        let result = write_token_file(
            &linked_dir.join("daemon.token"),
            &Token::new("secret".into()),
        );

        assert!(result.is_err());
        assert!(!target_dir.join("daemon.token").exists());
        let _ = fs::remove_file(linked_dir);
        let _ = fs::remove_dir(target_dir);
    }
}
