use clap::Parser;

use verbisage::cli::SharedArgs;
use verbisage::daemon::{DaemonConfig, DaemonHandler, run};

#[cfg(feature = "dbus")]
use verbisage::daemon::run_dbus;

#[derive(Parser)]
#[command(
    name = "verbisaged",
    version,
    about = "Word-list daemon — serves dictionary lookups over stdio JSON or D-Bus"
)]
struct DaemonCli {
    #[command(flatten)]
    shared: SharedArgs,
}

fn main() {
    let cli = DaemonCli::parse();
    let config = DaemonConfig::from_cli(&cli.shared);
    let handler = DaemonHandler::with_config(config);

    if cli.shared.dbus {
        #[cfg(feature = "dbus")]
        {
            let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| {
                eprintln!("failed to start tokio runtime: {}", e);
                std::process::exit(1);
            });
            if let Err(e) = rt.block_on(run_dbus(handler)) {
                eprintln!("dbus server error: {}", e);
            }
        }
        #[cfg(not(feature = "dbus"))]
        {
            let _ = handler;
            eprintln!("dbus feature not enabled; rebuild with --features dbus");
            std::process::exit(1);
        }
    } else {
        run(handler);
    }
}
