mod args;
mod cflags;
mod client;
mod config;
mod portage;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = args::Cli::parse_args();

    match cli.run().await {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("remerge: {e:#}");
            std::process::exit(1);
        }
    }
}
