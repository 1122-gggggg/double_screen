use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use splitdesk_core::{HostOs, SessionStatus};
use splitdesk_session::SessionRecord;
use splitdeskd::{
    load_token, rpc_with_token, DaemonCommand, DaemonResult, DiagnosticsScope, RpcError,
    DEFAULT_BIND,
};

#[derive(Parser, Debug)]
#[command(name = "splitdesk", about = "SplitDesk control CLI")]
pub struct Cli {
    /// Daemon JSON-lines address (loopback by default).
    #[arg(long, default_value = DEFAULT_BIND)]
    pub bind: String,

    /// Explicit daemon token (never logged).
    #[arg(long)]
    pub token: Option<String>,

    /// Token file; defaults to the platform daemon.token path.
    #[arg(long)]
    pub token_file: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    Status,
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    Metrics {
        #[arg(long)]
        session: Option<String>,
    },
    Diagnostics {
        #[command(subcommand)]
        cmd: Option<DiagnosticsCmd>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum SessionCmd {
    List,
    Create {
        #[arg(long)]
        user: String,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
        #[arg(long)]
        fps: Option<u32>,
        #[arg(long)]
        memory_limit: Option<String>,
        #[arg(long)]
        cpu_affinity: Option<String>,
    },
    Attach {
        id: String,
    },
    Destroy {
        id: String,
    },
    Info {
        id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum DiagnosticsCmd {
    Gpu,
    #[command(name = "media-path")]
    MediaPath,
}

pub fn command_to_daemon(cmd: &Command) -> DaemonCommand {
    match cmd {
        Command::Status => DaemonCommand::Status,
        Command::Session { cmd } => match cmd {
            SessionCmd::List => DaemonCommand::SessionList,
            SessionCmd::Create {
                user,
                width,
                height,
                fps,
                memory_limit,
                cpu_affinity,
            } => DaemonCommand::SessionCreate {
                user: user.clone(),
                width: *width,
                height: *height,
                fps: *fps,
                memory_limit: memory_limit.clone(),
                cpu_affinity: cpu_affinity.clone(),
            },
            SessionCmd::Attach { id } => DaemonCommand::Attach { id: id.clone() },
            SessionCmd::Destroy { id } => DaemonCommand::SessionDestroy { id: id.clone() },
            SessionCmd::Info { id } => DaemonCommand::SessionInfo { id: id.clone() },
        },
        Command::Metrics { session } => DaemonCommand::Metrics {
            id: session.clone(),
        },
        Command::Diagnostics { cmd } => DaemonCommand::Diagnostics {
            scope: match cmd {
                None => DiagnosticsScope::All,
                Some(DiagnosticsCmd::Gpu) => DiagnosticsScope::Gpu,
                Some(DiagnosticsCmd::MediaPath) => DiagnosticsScope::MediaPath,
            },
        },
    }
}

pub fn resolve_token(
    explicit: Option<&str>,
    token_file: Option<&Path>,
) -> Result<String, &'static str> {
    if let Some(token) = explicit {
        if token.trim().is_empty() {
            return Err("token required");
        }
        return Ok(token.trim().to_string());
    }
    match load_token(token_file) {
        Ok(token) => {
            let value = token.as_str().trim().to_string();
            if value.is_empty() {
                Err("token required")
            } else {
                Ok(value)
            }
        }
        Err(_) => Err("token required"),
    }
}

pub async fn run_cli(cli: Cli) -> Result<(), RpcError> {
    let token = resolve_token(cli.token.as_deref(), cli.token_file.as_deref())
        .map_err(|_| RpcError::TokenRequired)?;
    let cmd = command_to_daemon(&cli.cmd);
    let print_mode = match &cli.cmd {
        Command::Session {
            cmd: SessionCmd::List,
        } => PrintMode::SessionTable,
        Command::Diagnostics { cmd } => match cmd {
            None => PrintMode::Capabilities,
            Some(DiagnosticsCmd::Gpu) => PrintMode::Gpu,
            Some(DiagnosticsCmd::MediaPath) => PrintMode::MediaPath,
        },
        _ => PrintMode::Json,
    };
    let result = rpc_with_token(&cli.bind, &token, cmd).await?;
    print_result(result, print_mode);
    Ok(())
}

enum PrintMode {
    Json,
    SessionTable,
    Capabilities,
    Gpu,
    MediaPath,
}

fn print_result(result: DaemonResult, mode: PrintMode) {
    match (mode, result) {
        (PrintMode::SessionTable, DaemonResult::SessionList { sessions }) => {
            print_session_table(&sessions);
        }
        (PrintMode::Capabilities, DaemonResult::Diagnostics { capabilities, .. }) => {
            if let Some(caps) = capabilities {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&caps).unwrap_or_else(|_| "{}".into())
                );
            }
        }
        (PrintMode::Gpu, DaemonResult::Diagnostics { gpu, .. }) => {
            if let Some(gpu) = gpu {
                if let Some(text) = gpu
                    .as_object()
                    .and_then(|object| object.get("text"))
                    .and_then(serde_json::Value::as_str)
                {
                    println!("{text}");
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&gpu).unwrap_or_else(|_| "{}".into())
                    );
                }
            }
        }
        (PrintMode::MediaPath, DaemonResult::Diagnostics { media_path, .. }) => {
            if let Some(path) = media_path {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&path).unwrap_or_else(|_| "{}".into())
                );
            }
        }
        (_, result) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".into())
            );
        }
    }
}

pub fn print_session_table(sessions: &[SessionRecord]) {
    println!(
        "{:<10} {:<16} {:<8} {:<12} {:<5} {:<12}",
        "ID", "USER", "OS", "RESOLUTION", "FPS", "STATUS"
    );
    for session in sessions {
        println!(
            "{:<10} {:<16} {:<8} {:<12} {:<5} {:<12}",
            session.id.to_string(),
            session.user.to_string(),
            os_cell(session.os),
            format!("{}x{}", session.resolution.width, session.resolution.height),
            session.fps,
            status_cell(session.status)
        );
    }
}

fn os_cell(os: HostOs) -> &'static str {
    match os {
        HostOs::Linux => "Linux",
        HostOs::Windows => "Windows",
        HostOs::MacOs => "macOS",
        HostOs::Unknown => "Unknown",
    }
}

fn status_cell(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Starting => "Starting",
        SessionStatus::Running => "Running",
        SessionStatus::Detached => "Detached",
        SessionStatus::Connected => "Connected",
        SessionStatus::Stopping => "Stopping",
        SessionStatus::Failed => "Failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_status() {
        let cli = Cli::try_parse_from(["splitdesk", "status"]).unwrap();
        assert!(matches!(cli.cmd, Command::Status));
        assert_eq!(cli.bind, DEFAULT_BIND);
    }

    #[test]
    fn parse_session_list() {
        let cli = Cli::try_parse_from(["splitdesk", "session", "list"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Command::Session {
                cmd: SessionCmd::List
            }
        ));
    }

    #[test]
    fn parse_session_create_requires_user() {
        assert!(Cli::try_parse_from(["splitdesk", "session", "create"]).is_err());
        let cli =
            Cli::try_parse_from(["splitdesk", "session", "create", "--user", "alice"]).unwrap();
        match command_to_daemon(&cli.cmd) {
            DaemonCommand::SessionCreate { user, fps, .. } => {
                assert_eq!(user, "alice");
                assert_eq!(fps, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parse_session_attach_destroy_and_info() {
        let attach = Cli::try_parse_from(["splitdesk", "session", "attach", "sd-001"]).unwrap();
        let destroy = Cli::try_parse_from(["splitdesk", "session", "destroy", "sd-001"]).unwrap();
        let info = Cli::try_parse_from(["splitdesk", "session", "info", "sd-001"]).unwrap();
        assert!(matches!(
            command_to_daemon(&attach.cmd),
            DaemonCommand::Attach { id } if id == "sd-001"
        ));
        assert!(matches!(
            command_to_daemon(&destroy.cmd),
            DaemonCommand::SessionDestroy { id } if id == "sd-001"
        ));
        assert!(matches!(
            command_to_daemon(&info.cmd),
            DaemonCommand::SessionInfo { id } if id == "sd-001"
        ));
    }

    #[test]
    fn parse_diagnostics_scopes() {
        let all = Cli::try_parse_from(["splitdesk", "diagnostics"]).unwrap();
        let gpu = Cli::try_parse_from(["splitdesk", "diagnostics", "gpu"]).unwrap();
        let media = Cli::try_parse_from(["splitdesk", "diagnostics", "media-path"]).unwrap();
        assert!(matches!(
            command_to_daemon(&all.cmd),
            DaemonCommand::Diagnostics {
                scope: DiagnosticsScope::All
            }
        ));
        assert!(matches!(
            command_to_daemon(&gpu.cmd),
            DaemonCommand::Diagnostics {
                scope: DiagnosticsScope::Gpu
            }
        ));
        assert!(matches!(
            command_to_daemon(&media.cmd),
            DaemonCommand::Diagnostics {
                scope: DiagnosticsScope::MediaPath
            }
        ));
    }

    #[test]
    fn parse_metrics() {
        let cli = Cli::try_parse_from(["splitdesk", "metrics"]).unwrap();
        assert!(matches!(cli.cmd, Command::Metrics { session: None }));
    }

    #[test]
    fn missing_token_rejected() {
        assert_eq!(
            resolve_token(None, Some(Path::new("/no/such/token"))),
            Err("token required")
        );
        assert_eq!(resolve_token(Some(""), None), Err("token required"));
        assert_eq!(resolve_token(Some("   "), None), Err("token required"));
        assert_eq!(resolve_token(Some("abc"), None), Ok("abc".into()));
    }

    #[test]
    fn formats_all_platform_names() {
        assert_eq!(os_cell(HostOs::Linux), "Linux");
        assert_eq!(os_cell(HostOs::Windows), "Windows");
        assert_eq!(os_cell(HostOs::MacOs), "macOS");
        assert_eq!(os_cell(HostOs::Unknown), "Unknown");
    }
}
