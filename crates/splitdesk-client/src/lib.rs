use std::path::{Path, PathBuf};

use clap::Parser;
use splitdesk_client_core::WindowedClient;
use splitdesk_core::{Error as CoreError, SessionId};
use splitdeskd::{load_token, DEFAULT_BIND};

#[derive(Parser, Debug)]
#[command(
    name = "splitdesk-client",
    version,
    about = "Cross-platform SplitDesk native client"
)]
pub struct Args {
    /// Local SplitDesk daemon control address.
    #[arg(long, default_value = DEFAULT_BIND)]
    pub server: String,

    /// Loopback media endpoint override, useful when the SSH tunnel uses a non-default port.
    #[arg(long)]
    pub media_server: Option<String>,

    /// Host user whose desktop session should be attached.
    #[arg(long)]
    pub user: String,

    /// Existing session ID. Omit to create a new session.
    #[arg(long)]
    pub session: Option<String>,

    /// Start the client window in fullscreen mode.
    #[arg(long)]
    pub fullscreen: bool,

    /// Authentication token. Prefer --token-file so the token is not in process arguments.
    #[arg(long, conflicts_with = "token_file")]
    pub token: Option<String>,

    /// Authentication token file. Defaults to the platform-specific daemon token path.
    #[arg(long)]
    pub token_file: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientStartupError {
    #[error("inline token must not be blank")]
    BlankToken,
    #[error("could not load authentication token from {path}: {source}")]
    TokenFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid session id: {0}")]
    InvalidSession(String),
    #[error(transparent)]
    Client(#[from] CoreError),
}

pub fn platform_label() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    }
}

pub fn resolve_token(args: &Args) -> Result<String, ClientStartupError> {
    match args.token.as_deref() {
        Some(token) if token.trim().is_empty() => Err(ClientStartupError::BlankToken),
        Some(token) => Ok(token.to_owned()),
        None => {
            let path = args
                .token_file
                .clone()
                .unwrap_or_else(splitdeskd::default_token_path);
            load_token(Some(Path::new(&path)))
                .map(|token| token.as_str().to_owned())
                .map_err(|source| ClientStartupError::TokenFile { path, source })
        }
    }
}

pub async fn run(args: Args) -> Result<(), ClientStartupError> {
    let token = resolve_token(&args)?;
    let session = args
        .session
        .as_deref()
        .map(|id| {
            id.parse::<SessionId>()
                .map_err(|_| ClientStartupError::InvalidSession(id.to_owned()))
        })
        .transpose()?;

    let mut client = WindowedClient::new(args.server, args.user, token);
    client.set_fullscreen(args.fullscreen);
    client.set_media_server(args.media_server.as_deref())?;
    let id = client.connect(session).await?;
    println!("attached {id} on {}", platform_label());

    let window_result = client.run_loop();
    let disconnect_result = client.disconnect().await;
    window_result?;
    disconnect_result?;
    Ok(())
}

pub async fn run_from_env() -> Result<(), ClientStartupError> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .try_init();
    run(Args::parse()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_portable_client_arguments() {
        let args = Args::try_parse_from([
            "splitdesk-client",
            "--server",
            "[::1]:9823",
            "--media-server",
            "[::1]:19824",
            "--user",
            "測試 user",
            "--session",
            "sd-042",
            "--fullscreen",
            "--token-file",
            "/tmp/folder with spaces/token",
        ])
        .unwrap();

        assert_eq!(args.server, "[::1]:9823");
        assert_eq!(args.media_server.as_deref(), Some("[::1]:19824"));
        assert_eq!(args.user, "測試 user");
        assert_eq!(args.session.as_deref(), Some("sd-042"));
        assert!(args.fullscreen);
        assert_eq!(
            args.token_file.unwrap(),
            std::path::PathBuf::from("/tmp/folder with spaces/token")
        );
    }

    #[test]
    fn rejects_blank_inline_token_before_connecting() {
        let args = Args::try_parse_from(["splitdesk-client", "--user", "alice", "--token", "   "])
            .unwrap();

        assert!(matches!(
            resolve_token(&args),
            Err(ClientStartupError::BlankToken)
        ));
    }

    #[test]
    fn platform_label_is_never_empty() {
        assert!(!platform_label().is_empty());
    }
}
