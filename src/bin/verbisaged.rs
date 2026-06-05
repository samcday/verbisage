use clap::Parser;

use verbisage::cli::SharedArgs;
use verbisage::config::{default_config_path, load_config};
use verbisage::daemon::{BackendKind, DaemonConfig, DaemonHandler, run};
use verbisage::debug;
use verbisage::dictionary::paths::expand_tilde;
use verbisage::veprintln;

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

    veprintln!("config file: {}", config_path.as_ref().unwrap().display());
    veprintln!("backend: {:?}", cfg.backend);
    veprintln!("default language: {}", cfg.default_lang);
    if let Some(d) = &cfg.eager_system_dict {
        veprintln!("system dict override: {}", d);
    }
    if let Some(d) = &cfg.eager_user_dict {
        veprintln!("user dict override: {}", d);
    }
    if let Some(path) = &cfg.hunspell_affix {
        veprintln!("hunspell .aff: {}", path.display());
    }
    if let Some(path) = &cfg.hunspell_dict {
        veprintln!("hunspell .dic: {}", path.display());
    }
    veprintln!(
        "data dirs — system: {}, user: {}",
        cfg.language_paths.system_dir.display(),
        cfg.language_paths.user_dir.display(),
    );
    match cfg.backend {
        BackendKind::File => {
            veprintln!(
                "dict patterns — system: {:?}, user: {:?}",
                cfg.language_paths.system_dict_patterns,
                cfg.language_paths.user_dict_patterns,
            );
        }
        #[cfg(feature = "sqlite")]
        BackendKind::Sqlite => {
            veprintln!(
                "sqlite patterns — system: {:?}, user: {:?}",
                cfg.language_paths.system_sqlite_patterns,
                cfg.language_paths.user_sqlite_patterns,
            );
            veprintln!(
                "sqlite table/cols: {}/{}/{}",
                cfg.sqlite_table,
                cfg.sqlite_word_col,
                cfg.sqlite_freq_col,
            );
        }
        #[cfg(feature = "hunspell")]
        BackendKind::Hunspell => {}
    }

    let handler = DaemonHandler::with_config(cfg);

    if shared.dbus {
        veprintln!("transport: dbus (session bus)");
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
        veprintln!("transport: stdio");
        run(handler);
    }
}
