use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use splitdeskd::{run_daemon, DaemonConfig, DEFAULT_BIND, DEFAULT_IDLE_SECS};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "splitdeskd", about = "SplitDesk local control daemon")]
struct Args {
    /// Listen address. Default is loopback only.
    #[arg(long, default_value = DEFAULT_BIND)]
    bind: String,

    /// Token file path. Created at start with mode 0600 on Unix.
    #[arg(long)]
    token_file: Option<PathBuf>,

    /// Idle detach timeout in seconds.
    #[arg(long, default_value_t = DEFAULT_IDLE_SECS)]
    idle_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .json()
        .with_target(true)
        .init();

    let args = Args::parse();
    if args.bind.starts_with("0.0.0.0") {
        tracing::warn!(
            "bind override uses a non-loopback wildcard; default remains 127.0.0.1:9823"
        );
    }
    let config = DaemonConfig {
        bind: args.bind,
        token_path: args
            .token_file
            .unwrap_or_else(splitdeskd::default_token_path),
        idle: Duration::from_secs(args.idle_secs),
    };
    run_daemon(config).await
}
