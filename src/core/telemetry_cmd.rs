use anyhow::{Context, Result};
use clap::Subcommand;

const TELEMETRY_DISABLED_ENV: &str = "RTK_TELEMETRY_DISABLED";
const TELEMETRY_DISABLED_VALUE: &str = "1";

/// Label for the `device hash` row when no salt file exists.
///
/// A missing salt is only ever a real failure once every gate that stops the
/// salt from being written has been cleared. The arms below mirror the gate
/// order in `telemetry::maybe_ping`, so the label names the gate that actually
/// fired instead of blaming the filesystem. Issue #1656.
fn salt_missing_label(
    endpoint_configured: bool,
    env_override: bool,
    enabled: bool,
    consent_given: Option<bool>,
) -> &'static str {
    if !endpoint_configured {
        return "(not applicable; this build has no telemetry endpoint)";
    }
    if env_override {
        return "(blocked by RTK_TELEMETRY_DISABLED)";
    }
    match consent_given {
        // `Some(false)` (explicit opt-out) and `None` (never prompted) collapse
        // because the user-facing remediation — running `rtk telemetry enable`
        // — is identical in both states.
        Some(false) | None => "(telemetry not enabled; run `rtk telemetry enable` to opt in)",
        Some(true) if !enabled => "(telemetry disabled in config.toml)",
        // Every gate is clear, so the salt is simply not there yet. This is true
        // both when no ping has landed (the 23 h interval, or a run that touched
        // the marker before the detached thread wrote the salt) and when a write
        // genuinely failed — as far as the available state lets us go.
        Some(true) => "(no salt file yet; written on the first ping)",
    }
}

/// Format the full `  device hash:   ...` status line.
///
/// Extracted so the routing (which message string for which state) is covered
/// by unit tests rather than only the leaf label helper.
fn device_hash_line(
    endpoint_configured: bool,
    env_override: bool,
    enabled: bool,
    consent_given: Option<bool>,
    hash: Option<&str>,
) -> String {
    match hash {
        Some(h) if is_device_hash(h) => {
            format!("  device hash:   {}...{}", &h[..8], &h[56..])
        }
        // The caller only supplies a hash once the salt file exists, so a hash
        // that fails the shape check is a malformed salt, not a missing one.
        Some(_) => "  device hash:   (malformed device hash)".to_string(),
        None => format!(
            "  device hash:   {}",
            salt_missing_label(endpoint_configured, env_override, enabled, consent_given)
        ),
    }
}

/// A device hash is a SHA-256 digest rendered as lowercase hex: 64 ASCII
/// characters. Checking the alphabet as well as the length is what keeps
/// `&h[..8]` and `&h[56..]` on char boundaries — `len()` counts bytes, so a
/// 64-byte string of multi-byte characters would otherwise panic when sliced.
fn is_device_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

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
    let hash = if salt_path.exists() {
        Some(super::telemetry::generate_device_hash())
    } else {
        None
    };
    println!(
        "{}",
        device_hash_line(
            super::telemetry::endpoint_url().is_some(),
            env_override,
            config.telemetry.enabled,
            config.telemetry.consent_given,
            hash.as_deref(),
        )
    );

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
    let url = match super::telemetry::endpoint_url() {
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

    /// Regression for #1307: the env opt-out must short-circuit telemetry
    /// consent paths so `rtk init` cannot hang in non-interactive environments.
    /// All cases live in one test so the opt-out states cannot interleave.
    #[test]
    fn test_telemetry_disabled_by_env_honors_opt_out() {
        temp_env::with_var_unset(TELEMETRY_DISABLED_ENV, || {
            assert!(
                !telemetry_disabled_by_env(),
                "unset env must not count as disabled"
            );
        });

        temp_env::with_var(
            TELEMETRY_DISABLED_ENV,
            Some(TELEMETRY_DISABLED_VALUE),
            || {
                assert!(
                    telemetry_disabled_by_env(),
                    "RTK_TELEMETRY_DISABLED=1 must disable telemetry prompts (issue #1307)"
                );
            },
        );

        for other in ["0", "true", "false", "yes", "no", ""] {
            temp_env::with_var(TELEMETRY_DISABLED_ENV, Some(other), || {
                assert!(
                    !telemetry_disabled_by_env(),
                    "value {other:?} must not be treated as disabled"
                );
            });
        }
    }

    // A canned 64-hex-char hash for deterministic `device_hash_line` assertions.
    const SAMPLE_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn salt_missing_label_distinguishes_consent_not_given_from_real_failure() {
        // #1656: a fresh `brew install rtk` followed by `rtk telemetry status`
        // (without ever running `rtk init`) leaves `consent_given == None` and
        // therefore never writes a salt — that is by design, not a failure.
        // Surfacing it as `(no salt file)` reads as an error to users.
        let not_enabled = "(telemetry not enabled; run `rtk telemetry enable` to opt in)";
        assert_eq!(salt_missing_label(true, false, true, None), not_enabled);
        assert_eq!(
            salt_missing_label(true, false, true, Some(false)),
            not_enabled
        );
        // Every gate is clear and the salt is still missing, so the label says
        // what is actually known rather than accusing the filesystem.
        assert_eq!(
            salt_missing_label(true, false, true, Some(true)),
            "(no salt file yet; written on the first ping)"
        );
    }

    #[test]
    fn salt_missing_label_names_the_gate_that_actually_fired() {
        // Consent is one of five gates on salt creation, and three of the others
        // produce "consent granted, no salt" legitimately. The arms are ordered
        // as `telemetry::maybe_ping` applies its gates, so the first unmet gate
        // wins even when a later one is unmet too.
        assert_eq!(
            salt_missing_label(false, true, false, Some(false)),
            "(not applicable; this build has no telemetry endpoint)"
        );
        assert_eq!(
            salt_missing_label(true, true, false, Some(false)),
            "(blocked by RTK_TELEMETRY_DISABLED)"
        );
        assert_eq!(
            salt_missing_label(true, false, false, Some(true)),
            "(telemetry disabled in config.toml)"
        );
    }

    #[test]
    fn device_hash_line_renders_truncated_hash_when_salt_exists() {
        // The salt-exists path is gate-agnostic: once the hash is in hand it's
        // safe to show regardless of how the salt got there.
        let expected = "  device hash:   01234567...89abcdef";
        assert_eq!(
            device_hash_line(true, false, true, Some(true), Some(SAMPLE_HASH)),
            expected
        );
        assert_eq!(
            device_hash_line(false, true, false, None, Some(SAMPLE_HASH)),
            expected
        );
    }

    #[test]
    fn device_hash_line_routes_to_salt_missing_label_when_no_hash() {
        // Locks the routing: removing or inlining `salt_missing_label` without
        // copying its literals will fail these asserts.
        assert_eq!(
            device_hash_line(true, false, true, Some(true), None),
            "  device hash:   (no salt file yet; written on the first ping)"
        );
        assert_eq!(
            device_hash_line(true, false, true, None, None),
            "  device hash:   (telemetry not enabled; run `rtk telemetry enable` to opt in)"
        );
        assert_eq!(
            device_hash_line(true, false, true, Some(false), None),
            "  device hash:   (telemetry not enabled; run `rtk telemetry enable` to opt in)"
        );
        assert_eq!(
            device_hash_line(false, false, true, Some(true), None),
            "  device hash:   (not applicable; this build has no telemetry endpoint)"
        );
        assert_eq!(
            device_hash_line(true, true, true, Some(true), None),
            "  device hash:   (blocked by RTK_TELEMETRY_DISABLED)"
        );
        assert_eq!(
            device_hash_line(true, false, false, Some(true), None),
            "  device hash:   (telemetry disabled in config.toml)"
        );
    }

    #[test]
    fn device_hash_line_reports_a_malformed_hash_as_malformed() {
        // A hash only reaches here once the salt file exists, so "missing" would
        // be a claim the caller already knows to be false — the wrong length or
        // alphabet means the salt is malformed, not absent.
        let malformed = "  device hash:   (malformed device hash)";
        let short = "0123456789abcdef"; // 16 chars
        let long = SAMPLE_HASH.repeat(2); // 128 chars
        assert_eq!(
            device_hash_line(true, false, true, Some(true), Some(short)),
            malformed
        );
        assert_eq!(
            device_hash_line(true, false, true, Some(true), Some(&long)),
            malformed
        );
    }

    #[test]
    fn device_hash_line_rejects_a_64_byte_non_hex_hash_without_panicking() {
        // `len()` counts bytes: 21 three-byte characters plus one ASCII byte is
        // 64 bytes long, and slicing it at byte 8 would panic on a char boundary.
        let multibyte = format!("{}x", "日".repeat(21));
        assert_eq!(multibyte.len(), 64);
        assert_eq!(
            device_hash_line(true, false, true, Some(true), Some(&multibyte)),
            "  device hash:   (malformed device hash)"
        );
        // Right length and alphabet size, wrong alphabet.
        let non_hex = "z".repeat(64);
        assert_eq!(
            device_hash_line(true, false, true, Some(true), Some(&non_hex)),
            "  device hash:   (malformed device hash)"
        );
    }
}
