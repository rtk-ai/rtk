use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum TelemetrySubcommand {
    /// Show telemetry / local tracking status
    Status,
    /// Explain that remote uploads are unavailable (noop for remote consent)
    Enable,
    /// Clear legacy telemetry flags in config.toml
    Disable,
    /// Delete legacy device files and local tracking database
    Forget,
}

pub fn run(command: &TelemetrySubcommand) -> Result<()> {
    match command {
        TelemetrySubcommand::Status => run_status(),
        TelemetrySubcommand::Enable => run_enable(),
        TelemetrySubcommand::Disable => run_disable(),
        TelemetrySubcommand::Forget => run_forget(),
    }
}

fn run_status() -> Result<()> {
    let config = crate::core::config::Config::load().unwrap_or_default();

    println!("Telemetry (this build):");
    println!("  Remote uploads: disabled — RTK never sends usage data over the network.");
    println!(
        "  Local metrics:  SQLite (`rtk gain`, `rtk gain --history`) — see docs/usage/TRACKING.md"
    );
    println!();
    println!("Legacy [telemetry] section in config.toml (ignored for network):");

    let consent_str = match config.telemetry.consent_given {
        Some(true) => "yes",
        Some(false) => "no",
        None => "never asked",
    };
    println!("  consent:        {}", consent_str);
    if let Some(date) = &config.telemetry.consent_date {
        println!("  consent date:   {}", date);
    }
    println!(
        "  enabled field: {}",
        if config.telemetry.enabled {
            "yes"
        } else {
            "no"
        }
    );

    println!();
    let salt_path = super::telemetry::salt_file_path();
    let marker_path = super::telemetry::telemetry_marker_path();
    println!("Legacy local files:");
    println!(
        "  device salt:     {}",
        if salt_path.exists() {
            format!("{}", salt_path.display())
        } else {
            "(absent)".to_string()
        }
    );
    println!(
        "  ping marker:     {}",
        if marker_path.exists() {
            format!("{}", marker_path.display())
        } else {
            "(absent)".to_string()
        }
    );

    Ok(())
}

fn run_enable() -> Result<()> {
    println!(
        "This build does not upload usage telemetry. Command names and savings are recorded only on disk."
    );
    println!("See: rtk gain   (and docs/usage/TRACKING.md)");
    Ok(())
}

fn run_disable() -> Result<()> {
    crate::hooks::init::save_telemetry_consent(false)?;
    println!("Cleared legacy [telemetry] flags in config.toml (no remote pings in this build).");
    Ok(())
}

fn run_forget() -> Result<()> {
    crate::hooks::init::save_telemetry_consent(false)?;

    let salt_path = super::telemetry::salt_file_path();
    let marker_path = super::telemetry::telemetry_marker_path();

    if salt_path.exists() {
        std::fs::remove_file(&salt_path)
            .with_context(|| format!("Failed to delete {}", salt_path.display()))?;
        println!("Removed legacy salt file: {}", salt_path.display());
    }

    if marker_path.exists() {
        let _ = std::fs::remove_file(&marker_path);
    }

    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(super::constants::RTK_DATA_DIR)
        .join(super::constants::HISTORY_DB);
    if db_path.exists() {
        match std::fs::remove_file(&db_path) {
            Ok(()) => println!("Local tracking database deleted: {}", db_path.display()),
            Err(e) => eprintln!("rtk: could not delete {}: {}", db_path.display(), e),
        }
    }

    println!("Done. Remote telemetry was already disabled in this build.");
    Ok(())
}
