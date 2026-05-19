use remerge::args;
use remerge::status_bar::StatusBar;
use remerge::verbosity::Verbosity;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Detect verbosity early (before clap full-parse) so we can set RUST_LOG
    // before the tracing subscriber is initialised.
    let verbosity = Verbosity::early_detect();
    if std::env::var_os("RUST_LOG").is_none() {
        // SAFETY: we are single-threaded at this point (before tokio spawns workers).
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("RUST_LOG", verbosity.rust_log_level());
        }
    }

    let _telemetry = remerge_observability::init_tracing("remerge-cli", false)?;

    // Initialise the global status bar.  Must happen after the Tokio runtime
    // is up (needed to spawn the background redraw task).
    let _status_bar = StatusBar::init(verbosity.is_quiet());

    let cli = args::Cli::parse_args();

    match cli.run().await {
        Ok(()) => Ok(()),
        Err(e) => {
            // Clear the status bar before printing the error so it does not
            // overwrite the error message.
            if let Some(bar) = StatusBar::global() {
                bar.finish();
            }
            eprintln!("remerge: {e:#}");
            std::process::exit(1);
        }
    }
}
