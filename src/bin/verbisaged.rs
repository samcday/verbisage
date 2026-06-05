use clap::Parser;

use verbisage::cli::SharedArgs;
use verbisage::config::{default_config_path, load_config};
use verbisage::daemon::{DaemonConfig, DaemonHandler, run};
use verbisage::debug;
use verbisage::dictionary::paths::expand_tilde;

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
    debug::set_verbose(cli.shared.verbose);

    // Resolve config path: CLI override, then default.
    let config_path = cli
        .shared
        .config
        .clone()
        .map(|p| expand_tilde(p.to_str().unwrap_or("")))
        .or_else(|| Some(default_config_path()));
    let config = config_path.as_ref().and_then(load_config);

    let shared = cli.shared.apply_defaults(config.as_ref());
    let cfg = DaemonConfig::from_cli(&shared);
    let handler = DaemonHandler::with_config(cfg);

    if shared.dbus {
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
