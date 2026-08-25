use std::path::PathBuf;

use clap::Parser;
use splitdesk_client_core::WindowedClient;
use splitdesk_core::SessionId;
use splitdeskd::{load_token, DEFAULT_BIND};

#[derive(Parser, Debug)]
#[command(
    name = "splitdesk-client-linux",
    about = "SplitDesk Linux client (CLI attach + WindowedClient)"
)]
struct Args {
    #[arg(long, default_value = DEFAULT_BIND)]
    server: String,
    #[arg(long)]
    user: String,
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    fullscreen: bool,
    #[arg(long)]
    token: Option<String>,
    #[arg(long)]
    token_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    let args = Args::parse();
    let token = match args.token {
        Some(t) if !t.trim().is_empty() => t,
        Some(_) => anyhow::bail!("token required"),
        None => load_token(args.token_file.as_deref())
            .map(|t| t.as_str().to_string())
            .map_err(|_| anyhow::anyhow!("token required"))?,
    };

    let mut client = WindowedClient::new(args.server, args.user, token);
    client.set_fullscreen(args.fullscreen);
    let session = match args.session {
        Some(id) => Some(
            id.parse::<SessionId>()
                .map_err(|_| anyhow::anyhow!("bad session id"))?,
        ),
        None => None,
    };
    let id = client
        .connect(session)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("attached {id}");
    let run_result = client.run_loop();
    let disconnect_result = client.disconnect().await;
    run_result.map_err(|e| anyhow::anyhow!("{e}"))?;
    disconnect_result.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
