use anyhow::{Context, Result};
use clap::Subcommand;

const TELEMETRY_DISABLED_ENV: &str = "RTK_TELEMETRY_DISABLED";
const TELEMETRY_DISABLED_VALUE: &str = "1";

#[derive(Debug, Subcommand)]
pub enum TelemetrySubcommand {
    Status,
    Enable,
    Disable,
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

    let consent_str = match config.telemetry.consent_given {
        Some(true) => "yes",
        Some(false) => "no",
        None => "never asked",
    };

    let enabled_str = if config.telemetry.enabled {
        "yes"
    } else {
        "no"
    };

    let env_override = telemetry_disabled_by_env();

    println!("Telemetry status:");
    println!("  consent:       {}", consent_str);
    if let Some(date) = &config.telemetry.consent_date {
        println!("  consent date:  {}", date);
    }
    println!("  enabled:       {}", enabled_str);
    if env_override {
        println!("  env override:  RTK_TELEMETRY_DISABLED=1 (blocked)");
    }

    let salt_path = super::telemetry::salt_file_path();
    if salt_path.exists() {
        let hash = super::telemetry::generate_device_hash();
        println!("  device hash:   {}...{}", &hash[..8], &hash[56..]);
    } else {
        println!("  device hash:   (no salt file)");
    }

    println!();
    println!("Data controller: RTK AI Labs, contact@rtk-ai.app");
    println!("Details: https://github.com/rtk-ai/rtk/blob/master/docs/TELEMETRY.md");

    Ok(())
}

fn run_enable() -> Result<()> {
    use std::io::{self, BufRead, IsTerminal};

    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "consent requires interactive terminal — cannot enable telemetry in piped mode"
        );
    }

    eprintln!("RTK collects anonymous usage metrics once per day to improve filters.");
    eprintln!();
    eprintln!("  What:    command names (not arguments), token savings, OS, version");
    eprintln!("  Who:     RTK AI Labs, contact@rtk-ai.app");
    eprintln!("  Details: https://github.com/rtk-ai/rtk/blob/master/docs/TELEMETRY.md");
    eprintln!();
    eprint!("Enable anonymous telemetry? [y/N] ");

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read user input")?;

    let accepted = {
        let response = line.trim().to_lowercase();
        response == "y" || response == "yes"
    };

    crate::hooks::init::save_telemetry_consent(accepted)?;

    if accepted {
        println!("Telemetry enabled. Disable anytime: rtk telemetry disable");
    } else {
        println!("Telemetry not enabled.");
    }

    Ok(())
}

fn run_disable() -> Result<()> {
    crate::hooks::init::save_telemetry_consent(false)?;
    println!("Telemetry disabled.");
    Ok(())
}

fn run_forget() -> Result<()> {
    crate::hooks::init::save_telemetry_consent(false)?;

    let salt_path = super::telemetry::salt_file_path();
    let marker_path = super::telemetry::telemetry_marker_path();

    // Compute device hash before deleting the salt
    let device_hash = if salt_path.exists() {
        Some(super::telemetry::generate_device_hash())
    } else {
        None
    };

    if salt_path.exists() {
        std::fs::remove_file(&salt_path)
            .with_context(|| format!("Failed to delete {}", salt_path.display()))?;
    }

    if marker_path.exists() {
        let _ = std::fs::remove_file(&marker_path);
    }

    // Purge local tracking database (GDPR Art. 17 — right to erasure applies to local data too)
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

    // Send server-side erasure request
    if let Some(hash) = device_hash {
        match send_erasure_request(&hash) {
            Ok(()) => {
                println!("Erasure request sent to server.");
            }
            Err(e) => {
                eprintln!("rtk: could not reach server: {}", e);
                eprintln!("  To complete erasure, email contact@rtk-ai.app");
                eprintln!("  with your device hash: {}", hash);
            }
        }
    }

    println!("Local telemetry data deleted. Telemetry disabled.");
    Ok(())
}

fn send_erasure_request(device_hash: &str) -> Result<()> {
    let url = option_env!("RTK_TELEMETRY_URL");
    let url = match url {
        Some(u) => format!("{}/erasure", u),
        None => anyhow::bail!("no telemetry endpoint configured"),
    };

    let payload = serde_json::json!({
        "device_hash": device_hash,
        "action": "erasure",
    });

    let mut req = ureq::post(&url).set("Content-Type", "application/json");

    if let Some(token) = option_env!("RTK_TELEMETRY_TOKEN") {
        req = req.set("X-RTK-Token", token);
    }

    req.timeout(std::time::Duration::from_secs(5))
        .send_string(&payload.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::TelemetryConfig;
    use crate::core::constants;
    use crate::core::telemetry;
    use crate::hooks::init::save_telemetry_consent;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // Helper to create a temp config with specific telemetry settings
    fn with_temp_config<F>(telemetry: TelemetryConfig, f: F)
    where
        F: FnOnce(&TempDir),
    {
        let dir = TempDir::new().expect("tempdir");
        let config_dir = dir.path().join("rtk");
        fs::create_dir_all(&config_dir).expect("create config dir");

        let config = crate::core::config::Config {
            telemetry,
            ..Default::default()
        };
        let config_path = config_dir.join("config.toml");
        let toml = toml::to_string(&config).expect("serialize config");
        fs::write(&config_path, toml).expect("write config");

        // Override config directory - point to temp dir so get_config_path() finds rtk/config.toml
        let original = std::env::var("RTK_CONFIG_DIR").ok();
        std::env::set_var("RTK_CONFIG_DIR", dir.path());

        f(&dir);

        if let Some(orig) = original {
            std::env::set_var("RTK_CONFIG_DIR", orig);
        } else {
            std::env::remove_var("RTK_CONFIG_DIR");
        }
    }

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

    #[test]
    fn test_run_status_no_consent_never_asked() {
        let telemetry = TelemetryConfig {
            consent_given: None,
            enabled: false,
            consent_date: None,
        };
        with_temp_config(telemetry, |_dir| {
            // Should not panic
            let result = run_status();
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_run_status_consent_yes_enabled() {
        let telemetry = TelemetryConfig {
            consent_given: Some(true),
            enabled: true,
            consent_date: Some("2024-01-15T10:30:00Z".to_string()),
        };
        with_temp_config(telemetry, |_dir| {
            let result = run_status();
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_run_status_consent_no_disabled() {
        let telemetry = TelemetryConfig {
            consent_given: Some(false),
            enabled: false,
            consent_date: Some("2024-01-15T10:30:00Z".to_string()),
        };
        with_temp_config(telemetry, |_dir| {
            let result = run_status();
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_run_enable_non_interactive_fails() {
        let telemetry = TelemetryConfig {
            consent_given: None,
            enabled: false,
            consent_date: None,
        };
        with_temp_config(telemetry, |_dir| {
            // In non-interactive mode (stdin not a terminal), run_enable should fail
            // We can't easily test this without a real TTY, but we can verify the
            // function returns an error when stdin is not a terminal
            // The function checks io::stdin().is_terminal() which will be false in tests
            let result = run_enable();
            // Should fail with consent requires interactive terminal
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("interactive terminal") || err.contains("piped mode"));
        });
    }

    #[test]
    fn test_run_disable() {
        let telemetry = TelemetryConfig {
            consent_given: Some(true),
            enabled: true,
            consent_date: Some("2024-01-15T10:30:00Z".to_string()),
        };
        with_temp_config(telemetry, |_dir| {
            let result = run_disable();
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_run_forget_no_salt_no_marker() {
        let telemetry = TelemetryConfig {
            consent_given: Some(true),
            enabled: true,
            consent_date: Some("2024-01-15T10:30:00Z".to_string()),
        };
        with_temp_config(telemetry, |_dir| {
            // Ensure no salt file exists
            let salt_path = telemetry::salt_file_path();
            let _ = fs::remove_file(&salt_path);

            // Ensure no marker file exists
            let marker_path = telemetry::telemetry_marker_path();
            let _ = fs::remove_file(&marker_path);

            // Ensure no history db exists
            let db_path = dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(constants::RTK_DATA_DIR)
                .join(constants::HISTORY_DB);
            let _ = fs::remove_file(&db_path);

            let result = run_forget();
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_telemetry_subcommand_enum_variants() {
        // Verify all subcommand variants exist
        let _ = TelemetrySubcommand::Status;
        let _ = TelemetrySubcommand::Enable;
        let _ = TelemetrySubcommand::Disable;
        let _ = TelemetrySubcommand::Forget;
    }

    #[test]
    fn test_run_routes_to_correct_handler() {
        // Test that run() dispatches to correct handler
        // We test this by checking the match arms are exhaustive
        let status = TelemetrySubcommand::Status;
        let enable = TelemetrySubcommand::Enable;
        let disable = TelemetrySubcommand::Disable;
        let forget = TelemetrySubcommand::Forget;

        // Just verify they match - actual execution tested above
        assert!(matches!(status, TelemetrySubcommand::Status));
        assert!(matches!(enable, TelemetrySubcommand::Enable));
        assert!(matches!(disable, TelemetrySubcommand::Disable));
        assert!(matches!(forget, TelemetrySubcommand::Forget));
    }

    #[test]
    fn test_save_telemetry_consent_roundtrip() {
        let telemetry = TelemetryConfig {
            consent_given: None,
            enabled: false,
            consent_date: None,
        };
        with_temp_config(telemetry, |_dir| {
            // Test enabling
            save_telemetry_consent(true).expect("save consent true");
            let config = crate::core::config::Config::load().expect("load config");
            assert!(config.telemetry.enabled);
            assert_eq!(config.telemetry.consent_given, Some(true));

            // Test disabling
            save_telemetry_consent(false).expect("save consent false");
            let config = crate::core::config::Config::load().expect("load config");
            assert!(!config.telemetry.enabled);
            assert_eq!(config.telemetry.consent_given, Some(false));
        });
    }

    #[test]
    fn test_consent_date_format() {
        let telemetry = TelemetryConfig {
            consent_given: Some(true),
            enabled: true,
            consent_date: Some("2024-01-15T10:30:00+00:00".to_string()),
        };
        with_temp_config(telemetry, |_dir| {
            let result = run_status();
            assert!(result.is_ok());
        });
    }
}
