use crate::core::config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLanguage {
    En,
    Id,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    TelemetryStatusHeader,
    TelemetryStatusConsentLabel,
    TelemetryStatusConsentDateLabel,
    TelemetryStatusEnabledLabel,
    TelemetryStatusEnvOverrideLabel,
    TelemetryStatusDeviceHashLabel,
    TelemetryStatusMissingSalt,
    TelemetryStatusDataController,
    TelemetryStatusDetails,
    TelemetryStatusBlocked,
    TelemetryEnableNeedsTerminal,
    TelemetryEnableIntro,
    TelemetryEnableWhat,
    TelemetryEnableWhy,
    TelemetryEnableWho,
    TelemetryEnableRights,
    TelemetryEnableRightsErasure,
    TelemetryEnableDetails,
    TelemetryEnableQuestion,
    TelemetryEnableEnabled,
    TelemetryEnableDisabled,
    TelemetryPromptHeader,
    ConsentUnknown,
}

pub fn ui_language(config: &config::Config) -> UiLanguage {
    match language_hint(config).as_str() {
        "id" => UiLanguage::Id,
        _ => UiLanguage::En,
    }
}

pub fn language_hint(config: &config::Config) -> String {
    let raw = language_candidate(config)
        .as_deref()
        .unwrap_or("en")
        .to_string();

    match raw.as_str() {
        "en" | "id" | "es" | "fr" | "ja" | "ko" | "zh" | "ru" | "de" => raw,
        _ => "other".to_string(),
    }
}

pub fn t(message: Message, lang: UiLanguage) -> &'static str {
    match lang {
        UiLanguage::En => match message {
            Message::TelemetryStatusHeader => "Telemetry status:",
            Message::TelemetryStatusConsentLabel => "  consent:",
            Message::TelemetryStatusConsentDateLabel => "  consent date:",
            Message::TelemetryStatusEnabledLabel => "  enabled:",
            Message::TelemetryStatusEnvOverrideLabel => "  env override:",
            Message::TelemetryStatusDeviceHashLabel => "  device hash:",
            Message::TelemetryStatusMissingSalt => "(no salt file)",
            Message::TelemetryStatusDataController => "Data controller: RTK AI Labs, contact@rtk-ai.app",
            Message::TelemetryStatusDetails => "Details: https://github.com/rtk-ai/rtk/blob/main/docs/TELEMETRY.md",
            Message::TelemetryStatusBlocked => "(blocked)",
            Message::TelemetryEnableNeedsTerminal => {
                "consent requires interactive terminal — cannot enable telemetry in piped mode"
            }
            Message::TelemetryEnableIntro => {
                "RTK collects anonymous usage metrics once per day to improve filters."
            }
            Message::TelemetryEnableWhat => {
                "  What:    command names (not arguments), token savings, OS, version"
            }
            Message::TelemetryEnableWhy => {
                "  Why:     prioritize filter development for the most-used commands"
            }
            Message::TelemetryEnableWho => "  Who:     RTK AI Labs, contact@rtk-ai.app",
            Message::TelemetryEnableRights => {
                "  Rights:  disable anytime with `rtk telemetry disable`,"
            }
            Message::TelemetryEnableRightsErasure => {
                "           request erasure with `rtk telemetry forget`"
            }
            Message::TelemetryEnableDetails => {
                "  Details: https://github.com/rtk-ai/rtk/blob/main/docs/TELEMETRY.md"
            }
            Message::TelemetryEnableQuestion => "Enable anonymous telemetry? [y/N] ",
            Message::TelemetryEnableEnabled => {
                "  Telemetry enabled. Disable anytime: rtk telemetry disable"
            }
            Message::TelemetryEnableDisabled => "  Telemetry not enabled.",
            Message::TelemetryPromptHeader => "\n--- Telemetry ---",
            Message::ConsentUnknown => "never asked",
        },
        UiLanguage::Id => match message {
            Message::TelemetryStatusHeader => "Status telemetri:",
            Message::TelemetryStatusConsentLabel => "  persetujuan:",
            Message::TelemetryStatusConsentDateLabel => "  tanggal persetujuan:",
            Message::TelemetryStatusEnabledLabel => "  aktif:",
            Message::TelemetryStatusEnvOverrideLabel => "  env override:",
            Message::TelemetryStatusDeviceHashLabel => "  hash perangkat:",
            Message::TelemetryStatusMissingSalt => "(tidak ada file garam)",
            Message::TelemetryStatusDataController => {
                "Pengendali data: RTK AI Labs, contact@rtk-ai.app"
            }
            Message::TelemetryStatusDetails => {
                "Rincian: https://github.com/rtk-ai/rtk/blob/main/docs/TELEMETRY.md"
            }
            Message::TelemetryStatusBlocked => "(diblokir)",
            Message::TelemetryEnableNeedsTerminal => {
                "persetujuan memerlukan terminal interaktif — tidak bisa diaktifkan lewat mode pipe"
            }
            Message::TelemetryEnableIntro => {
                "RTK mengumpulkan metrik penggunaan anonim setiap hari untuk meningkatkan filter."
            }
            Message::TelemetryEnableWhat => {
                "  Apa:    nama perintah (bukan argumen), penghematan token, OS, versi"
            }
            Message::TelemetryEnableWhy => {
                "  Mengapa: memprioritaskan pengembangan filter untuk perintah paling sering digunakan"
            }
            Message::TelemetryEnableWho => "  Siapa: RTK AI Labs, contact@rtk-ai.app",
            Message::TelemetryEnableRights => {
                "  Hak:    nonaktifkan kapan pun dengan `rtk telemetry disable`,"
            }
            Message::TelemetryEnableRightsErasure => {
                "         minta penghapusan dengan `rtk telemetry forget`"
            }
            Message::TelemetryEnableDetails => {
                "  Rincian: https://github.com/rtk-ai/rtk/blob/main/docs/TELEMETRY.md"
            }
            Message::TelemetryEnableQuestion => "Aktifkan telemetri anonim? [y/N] ",
            Message::TelemetryEnableEnabled => {
                "  Telemetri diaktifkan. Nonaktifkan kapan pun: rtk telemetry disable"
            }
            Message::TelemetryEnableDisabled => "  Telemetri tidak aktif.",
            Message::TelemetryPromptHeader => "\n--- Telemetri ---",
            Message::ConsentUnknown => "belum ditanya",
        },
    }
}

pub fn bool_text(value: bool, lang: UiLanguage) -> &'static str {
    match (value, lang) {
        (true, UiLanguage::Id) => "ya",
        (false, UiLanguage::Id) => "tidak",
        (true, _) => "yes",
        (false, _) => "no",
    }
}

fn language_candidate(config: &config::Config) -> Option<String> {
    const CANDIDATE_ENV: [&str; 4] = ["RTK_DISPLAY_LANGUAGE", "LANGUAGE", "LC_MESSAGES", "LANG"];

    if let Some(candidate) = CANDIDATE_ENV.iter().find_map(|env_name| {
        if let Ok(value) = std::env::var(env_name) {
            if let Some(candidate) = normalize(&value) {
                return Some(candidate);
            }
        }
        None
    }) {
        return Some(candidate);
    }

    normalize(&config.display.language)
}

fn normalize(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("C") {
        return None;
    }

    let value = value
        .split(':')
        .next()
        .unwrap_or_default()
        .split('.')
        .next()
        .unwrap_or_default();

    let value = value
        .split('@')
        .next()
        .unwrap_or_default()
        .replace('_', "-");

    let lang = value
        .split('-')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();

    if lang.len() == 2 && lang.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(lang)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_language_default_is_english() {
        let cfg = config::Config::default();
        assert_eq!(ui_language(&cfg), UiLanguage::En);
    }

    #[test]
    fn test_language_hint_from_display_language_config() {
        let mut cfg = config::Config::default();
        cfg.display.language = "id".to_string();

        assert_eq!(language_hint(&cfg), "id");
    }

    #[test]
    fn test_language_hint_normalizes_locales() {
        let mut cfg = config::Config::default();
        cfg.display.language = "ID_ID.UTF-8".to_string();

        assert_eq!(language_hint(&cfg), "id");
    }

    #[test]
    fn test_language_hint_unknown_lang_becomes_other() {
        let mut cfg = config::Config::default();
        cfg.display.language = "xx_YY.UTF-8".to_string();

        assert_eq!(language_hint(&cfg), "other");
    }

    #[test]
    fn test_bool_text_is_idiomatic() {
        assert_eq!(bool_text(true, UiLanguage::En), "yes");
        assert_eq!(bool_text(false, UiLanguage::En), "no");
        assert_eq!(bool_text(true, UiLanguage::Id), "ya");
        assert_eq!(bool_text(false, UiLanguage::Id), "tidak");
    }

    #[test]
    fn test_prompt_message_is_localized() {
        assert_eq!(t(Message::TelemetryPromptHeader, UiLanguage::En), "\n--- Telemetry ---");
        assert_eq!(t(Message::TelemetryPromptHeader, UiLanguage::Id), "\n--- Telemetri ---");
    }

    #[test]
    fn test_language_hint_from_environment_fallback_and_precedence() {
        let mut cfg = config::Config::default();
        cfg.display.language = "".to_string();

        let keys = ["RTK_DISPLAY_LANGUAGE", "LANGUAGE", "LC_MESSAGES", "LANG"];
        let backups: Vec<(String, Option<String>)> = keys
            .iter()
            .map(|key| (key.to_string(), std::env::var(key).ok()))
            .collect();

        for key in keys {
            std::env::remove_var(key);
        }

        std::env::set_var("LANGUAGE", "id_ID.UTF-8");
        assert_eq!(language_hint(&cfg), "id");

        std::env::set_var("RTK_DISPLAY_LANGUAGE", "es");
        assert_eq!(language_hint(&cfg), "es");

        std::env::set_var("RTK_DISPLAY_LANGUAGE", "C");
        std::env::set_var("LANG", "id_ID.UTF-8");
        assert_eq!(language_hint(&cfg), "id");

        for (key, value) in backups {
            if let Some(value) = value {
                std::env::set_var(&key, value);
            } else {
                std::env::remove_var(&key);
            }
        }
    }

    #[test]
    fn test_language_hint_prefers_environment_over_config() {
        let mut cfg = config::Config::default();
        cfg.display.language = "id".to_string();

        std::env::set_var("RTK_DISPLAY_LANGUAGE", "en");
        std::env::remove_var("LANGUAGE");
        std::env::remove_var("LC_MESSAGES");
        std::env::remove_var("LANG");

        assert_eq!(language_hint(&cfg), "en");

        std::env::set_var("RTK_DISPLAY_LANGUAGE", "C");
        std::env::set_var("LANG", "id_ID.UTF-8");
        assert_eq!(language_hint(&cfg), "id");

        cfg.display.language = "".to_string();
        std::env::set_var("RTK_DISPLAY_LANGUAGE", "id");
        std::env::remove_var("LANGUAGE");
        std::env::remove_var("LC_MESSAGES");
        std::env::remove_var("LANG");
        assert_eq!(language_hint(&cfg), "id");

        std::env::remove_var("RTK_DISPLAY_LANGUAGE");
        std::env::set_var("LANG", "id_ID.UTF-8");
        assert_eq!(language_hint(&cfg), "id");

        std::env::remove_var("LANG");
    }
}
