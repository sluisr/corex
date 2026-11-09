#[tokio::main]
async fn main() -> anyhow::Result<()> {
    corex_browser::run_native_host().await
}
