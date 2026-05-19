use remerge::args;
use remerge::status_bar::StatusBar;
use remerge::verbosity::Verbosity;

use anyhow::Result;

fn main() -> Result<()> {
    // Detect verbosity early (before clap full-parse) so we can set RUST_LOG
    // before the tracing subscriber and Tokio runtime are initialised.
    // This *must* happen in a single-threaded synchronous context — before
    // any thread is spawned — because std::env::set_var is unsafe from Rust
    // 1.92 onward when concurrent threads might be reading the environment.
    let verbosity = Verbosity::early_detect();
    if std::env::var_os("RUST_LOG").is_none() {
        // SAFETY: we are still single-threaded here — no Tokio runtime or
        // other threads have been spawned yet.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("RUST_LOG", verbosity.rust_log_level());
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(verbosity))
}

async fn async_main(verbosity: Verbosity) -> Result<()> {
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
