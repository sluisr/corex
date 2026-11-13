#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    corex_cli::run().await
}
