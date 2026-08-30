#[tokio::main]
async fn main() -> Result<(), splitdesk_client::ClientStartupError> {
    splitdesk_client::run_from_env().await
}
