use remerge::args;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = remerge_observability::init_tracing("remerge-cli", false)?;

    let cli = args::Cli::parse_args();

    match cli.run().await {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("remerge: {e:#}");
            std::process::exit(1);
        }
    }
}
