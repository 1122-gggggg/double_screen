use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use splitdeskd::{run_daemon, DaemonConfig, DEFAULT_BIND, DEFAULT_IDLE_SECS, DEFAULT_MEDIA_BIND};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "splitdeskd", about = "SplitDesk local control daemon")]
struct Args {
    /// Listen address. Default is loopback only.
    #[arg(long, default_value = DEFAULT_BIND)]
    bind: String,

    /// Media plane listen address. Loopback is required until encrypted transport is available.
    #[arg(long, default_value = DEFAULT_MEDIA_BIND)]
    media_bind: String,

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
    let config = DaemonConfig {
        bind: args.bind,
        media_bind: args.media_bind,
        token_path: args
            .token_file
            .unwrap_or_else(splitdeskd::default_token_path),
        idle: Duration::from_secs(args.idle_secs),
    };
    run_daemon(config).await
}
