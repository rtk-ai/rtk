use anyhow::{Context, Result};
use clap::Subcommand;

const TELEMETRY_DISABLED_ENV: &str = "RTK_TELEMETRY_DISABLED";
const TELEMETRY_DISABLED_VALUE: &str = "1";

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

/// Returns true when telemetry is explicitly disabled through the
/// `RTK_TELEMETRY_DISABLED` env var (value `"1"`).
///
/// Single source of truth for the env opt-out so the consent prompt
/// (`init::prompt_telemetry_consent`), the status command, and
/// `telemetry::maybe_ping` never diverge — if the accepted values ever grow
/// (e.g. `"true"`, `"y"`), they change here once.
pub fn telemetry_disabled_by_env() -> bool {
    std::env::var(TELEMETRY_DISABLED_ENV).unwrap_or_default() == TELEMETRY_DISABLED_VALUE
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for #1307: the env opt-out must short-circuit telemetry
    /// consent paths so `rtk init` cannot hang in non-interactive environments.
    /// All cases are bundled in one test to serialize env-var mutations.
    #[test]
    fn test_telemetry_disabled_by_env_honors_opt_out() {
        #[allow(deprecated)]
        std::env::remove_var(TELEMETRY_DISABLED_ENV);
        assert!(
            !telemetry_disabled_by_env(),
            "unset env must not count as disabled"
        );

        #[allow(deprecated)]
        std::env::set_var(TELEMETRY_DISABLED_ENV, TELEMETRY_DISABLED_VALUE);
        assert!(
            telemetry_disabled_by_env(),
            "RTK_TELEMETRY_DISABLED=1 must disable telemetry prompts (issue #1307)"
        );

        for other in ["0", "true", "false", "yes", "no", ""] {
            #[allow(deprecated)]
            std::env::set_var(TELEMETRY_DISABLED_ENV, other);
            assert!(
                !telemetry_disabled_by_env(),
                "value {other:?} must not be treated as disabled"
            );
        }

        #[allow(deprecated)]
        std::env::remove_var(TELEMETRY_DISABLED_ENV);
    }
}
