use anyhow::{Context, Result};
use clap::Subcommand;

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

fn run_status() -> Result<()> {
    let config = crate::core::config::Config::load().unwrap_or_default();
    let lang = crate::core::i18n::ui_language(&config);

    let consent_str = match config.telemetry.consent_given {
        Some(true) => crate::core::i18n::bool_text(true, lang),
        Some(false) => crate::core::i18n::bool_text(false, lang),
        None => crate::core::i18n::t(crate::core::i18n::Message::ConsentUnknown, lang),
    };

    let enabled_str = if config.telemetry.enabled {
        crate::core::i18n::bool_text(true, lang)
    } else {
        crate::core::i18n::bool_text(false, lang)
    };

    let env_override = std::env::var("RTK_TELEMETRY_DISABLED").unwrap_or_default() == "1";

    println!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryStatusHeader, lang)
    );
    println!(
        "{} {}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryStatusConsentLabel, lang),
        consent_str
    );
    if let Some(date) = &config.telemetry.consent_date {
        println!(
            "{} {}",
            crate::core::i18n::t(
                crate::core::i18n::Message::TelemetryStatusConsentDateLabel,
                lang
            ),
            date
        );
    }
    println!(
        "{} {}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryStatusEnabledLabel, lang),
        enabled_str
    );
    if env_override {
        println!(
            "{} RTK_TELEMETRY_DISABLED=1 {}",
            crate::core::i18n::t(
                crate::core::i18n::Message::TelemetryStatusEnvOverrideLabel,
                lang
            ),
            crate::core::i18n::t(crate::core::i18n::Message::TelemetryStatusBlocked, lang)
        );
    }

    let salt_path = super::telemetry::salt_file_path();
    if salt_path.exists() {
        let hash = super::telemetry::generate_device_hash();
        println!(
            "{} {}...{}",
            crate::core::i18n::t(
                crate::core::i18n::Message::TelemetryStatusDeviceHashLabel,
                lang
            ),
            &hash[..8],
            &hash[56..]
        );
    } else {
        println!(
            "{} {}",
            crate::core::i18n::t(
                crate::core::i18n::Message::TelemetryStatusDeviceHashLabel,
                lang
            ),
            crate::core::i18n::t(
                crate::core::i18n::Message::TelemetryStatusMissingSalt,
                lang
            )
        );
    }

    println!();
    println!(
        "{}",
        crate::core::i18n::t(
            crate::core::i18n::Message::TelemetryStatusDataController,
            lang
        )
    );
    println!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryStatusDetails, lang)
    );

    Ok(())
}

fn run_enable() -> Result<()> {
    use std::io::{self, BufRead, IsTerminal};

    let config = crate::core::config::Config::load().unwrap_or_default();
    let lang = crate::core::i18n::ui_language(&config);

    if !io::stdin().is_terminal() {
        anyhow::bail!(
            crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableNeedsTerminal, lang)
        );
    }

    eprintln!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableIntro, lang)
    );
    eprintln!();
    eprintln!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableWhat, lang)
    );
    eprintln!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableWhy, lang)
    );
    eprintln!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableWho, lang)
    );
    eprintln!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableRights, lang)
    );
    eprintln!(
        "{}",
        crate::core::i18n::t(
            crate::core::i18n::Message::TelemetryEnableRightsErasure,
            lang
        )
    );
    eprintln!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableDetails, lang)
    );
    eprintln!();
    eprint!(
        "{}",
        crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableQuestion, lang)
    );

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
        println!(
            "{}",
            crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableEnabled, lang)
        );
    } else {
        println!(
            "{}",
            crate::core::i18n::t(crate::core::i18n::Message::TelemetryEnableDisabled, lang)
        );
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
