use clap::Parser;
use splitdesk_cli::{run_cli, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    run_cli(cli).await.map_err(|err| anyhow::anyhow!("{err}"))
}
