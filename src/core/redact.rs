//! PII redaction: emails, phone numbers, Indian PAN, Aadhaar, card numbers, secrets.
//!
//! Applied centrally where raw command output is first captured (see
//! `core::stream::exec_capture`, `core::stream::run_streaming`, `run_fallback`
//! and the `rtk proxy` streaming loop in `main.rs`), so every rtk output path
//! is covered without touching each of the ~70 `cmds/**` filter modules.
//! Because redaction runs before `guard::never_worse` ever sees the strings,
//! the never-worse guard can never resurrect raw PII by picking the raw side.
//!
//! ON by default; disable via `[redaction] enabled = false` in
//! `~/.config/rtk/config.toml` or the global `--no-redact` flag.

use std::borrow::Cow;
use std::io::{Read, Write};
use std::sync::OnceLock;

use lazy_static::lazy_static;
use regex::{Regex, RegexSet};

use crate::core::config::{Config, RedactionConfig};

lazy_static! {
    static ref EMAIL_RE: Regex =
        Regex::new(r"(?i)\b[a-z0-9][a-z0-9._%+\-]*@[a-z0-9][a-z0-9.\-]*\.[a-z]{2,}\b").unwrap();

    // Indian mobile: optional +91 prefix, 10 digits starting 6-9.
    static ref PHONE_IN_RE: Regex =
        Regex::new(r"(?:\+91[\s\-]?)?\b[6-9]\d{9}\b").unwrap();

    // Generic phone: only high-confidence shapes — a (area) prefix or a +CC
    // prefix with grouped digits. Bare pairs like "1234 5678" are deliberately
    // NOT matched: columnar numeric CLI output would false-positive constantly.
    static ref PHONE_GENERIC_RE: Regex = Regex::new(
        r"\b(?:\+\d{1,3}[\s\-]?)?\(\d{2,4}\)[\s\-]?\d{3,4}[\s\-]?\d{3,4}\b|\+\d{1,3}[\s\-]\d{3,4}[\s\-]\d{3,4}(?:[\s\-]\d{2,4})?\b"
    ).unwrap();

    static ref PAN_RE: Regex = Regex::new(r"\b[A-Z]{5}[0-9]{4}[A-Z]\b").unwrap();

    // Aadhaar candidate: 12 digits, optionally 4-4-4 grouped. Verhoeff-gated in code.
    static ref AADHAAR_RE: Regex =
        Regex::new(r"\b\d{4}[\s\-]?\d{4}[\s\-]?\d{4}\b").unwrap();

    // Card candidate: 13-19 digits with optional single space/dash separators,
    // starting and ending on a digit. Luhn-gated in code.
    static ref CARD_RE: Regex = Regex::new(r"\b\d(?:[ \-]?\d){12,18}\b").unwrap();

    // AKIA = long-term access key, ASIA = temporary (STS) access key.
    static ref AWS_KEY_RE: Regex = Regex::new(r"\bA(?:KIA|SIA)[0-9A-Z]{16}\b").unwrap();

    // IPv4 candidate: 4 dotted octets, not embedded in a longer dotted run
    // (so semver "1.2.3" never matches and "1.2.3.4.5" is left alone).
    // Octet range + loopback/unspecified skips are validated in code.
    static ref IPV4_RE: Regex =
        Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap();
    static ref JWT_RE: Regex = Regex::new(
        r"\bey[A-Za-z0-9_-]{10,}\.ey[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b"
    ).unwrap();
    static ref BEARER_RE: Regex =
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9\-._~+/]+=*").unwrap();
    static ref PEM_RE: Regex = Regex::new(
        r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----"
    ).unwrap();
    static ref KV_SECRET_RE: Regex = Regex::new(
        r#"(?i)\b(api[_-]?key|password|passwd|secret|access[_-]?key|auth[_-]?token)\s*[:=]\s*['"]?[^\s'",;]+['"]?"#
    ).unwrap();

    // Fast reject: if none of these match, the input is clean and the whole
    // pipeline is skipped (single scan, no allocation).
    static ref PRECHECK_SET: RegexSet = RegexSet::new([
        EMAIL_RE.as_str(),
        PHONE_IN_RE.as_str(),
        PHONE_GENERIC_RE.as_str(),
        PAN_RE.as_str(),
        AADHAAR_RE.as_str(),
        CARD_RE.as_str(),
        AWS_KEY_RE.as_str(),
        IPV4_RE.as_str(),
        JWT_RE.as_str(),
        BEARER_RE.as_str(),
        r"-----BEGIN",
        KV_SECRET_RE.as_str(),
    ]).unwrap();
}

/// Standard Luhn checksum over an ASCII-digit string.
fn luhn_valid(digits: &str) -> bool {
    let sum: u32 = digits
        .chars()
        .rev()
        .enumerate()
        .map(|(i, c)| {
            let d = c.to_digit(10).unwrap_or(0);
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

// Standard Verhoeff tables (dihedral group D5) — see Wikipedia "Verhoeff algorithm".
const VERHOEFF_D: [[u8; 10]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
    [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
    [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
    [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
    [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
    [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
    [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
    [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
    [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
];
const VERHOEFF_P: [[u8; 10]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
    [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
    [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
    [9, 4, 5, 3, 1, 2, 6, 8, 7, 0],
    [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
    [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
    [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
];

/// Verhoeff checksum validation (Aadhaar check digit scheme).
fn verhoeff_valid(digits: &str) -> bool {
    let mut c = 0u8;
    for (i, ch) in digits.chars().rev().enumerate() {
        let d = match ch.to_digit(10) {
            Some(d) => d as usize,
            None => return false,
        };
        c = VERHOEFF_D[c as usize][VERHOEFF_P[i % 8][d] as usize];
    }
    c == 0
}

pub struct Redactor {
    enabled: bool,
    email: bool,
    phone: bool,
    pan: bool,
    aadhaar: bool,
    card: bool,
    secrets: bool,
    ip: bool,
    custom: Vec<(String, Regex)>,
    allowlist: Vec<Regex>,
}

static NO_REDACT_FLAG: OnceLock<bool> = OnceLock::new();
static GLOBAL: OnceLock<Redactor> = OnceLock::new();

/// Record the `--no-redact` CLI flag. Call once in `main()` right after
/// `Cli::parse()`, before any command dispatch.
pub fn init_from_cli(no_redact: bool) {
    NO_REDACT_FLAG.set(no_redact).ok();
}

/// Redact all configured PII categories from `input` using the process-wide
/// redactor (config + CLI flag, built once). `Cow::Borrowed` when clean.
pub fn redact(input: &str) -> Cow<'_, str> {
    global().redact(input)
}

/// Owned-line variant for streaming loops: reuses the incoming allocation when
/// the line is clean instead of copying it.
pub fn redact_line(line: String) -> String {
    match global().redact(&line) {
        Cow::Borrowed(_) => line,
        Cow::Owned(o) => o,
    }
}

fn global() -> &'static Redactor {
    GLOBAL.get_or_init(|| {
        let cfg = Config::load().map(|c| c.redaction).unwrap_or_default();
        Redactor::from_config(&cfg, *NO_REDACT_FLAG.get().unwrap_or(&false))
    })
}

impl Redactor {
    pub fn from_config(cfg: &RedactionConfig, no_redact_flag: bool) -> Self {
        let compile = |name: &str, pattern: &str| -> Option<Regex> {
            match Regex::new(pattern) {
                Ok(re) => Some(re),
                Err(e) => {
                    eprintln!("[rtk] warning: invalid redaction pattern '{}': {}", name, e);
                    None
                }
            }
        };
        Self {
            enabled: cfg.enabled && !no_redact_flag,
            email: cfg.email,
            phone: cfg.phone,
            pan: cfg.pan,
            aadhaar: cfg.aadhaar,
            card: cfg.card,
            secrets: cfg.secrets,
            ip: cfg.ip,
            custom: cfg
                .custom
                .iter()
                .filter_map(|c| compile(&c.name, &c.pattern).map(|re| (c.name.clone(), re)))
                .collect(),
            allowlist: cfg
                .allowlist
                .iter()
                .filter_map(|p| compile("allowlist", p))
                .collect(),
        }
    }

    pub fn redact<'a>(&self, input: &'a str) -> Cow<'a, str> {
        if !self.enabled {
            return Cow::Borrowed(input);
        }
        if !PRECHECK_SET.is_match(input) && self.custom.is_empty() {
            return Cow::Borrowed(input);
        }

        let out = if self.allowlist.is_empty() {
            self.redact_blob(input)
        } else {
            // Allowlist is line-scoped: PEM blocks (multi-line) are redacted
            // first on the whole blob, then every non-allowlisted line goes
            // through the remaining single-line stages.
            let pem_done: Cow<'_, str> = if self.secrets {
                PEM_RE.replace_all(input, "[REDACTED:private_key]")
            } else {
                Cow::Borrowed(input)
            };
            let mut result = String::with_capacity(pem_done.len());
            for segment in pem_done.split_inclusive('\n') {
                if self.allowlist.iter().any(|re| re.is_match(segment)) {
                    result.push_str(segment);
                } else {
                    result.push_str(&self.redact_blob(segment));
                }
            }
            result
        };

        if out == input {
            Cow::Borrowed(input)
        } else {
            Cow::Owned(out)
        }
    }

    /// Sequential replace pipeline, most-specific first. Each stage runs on the
    /// previous stage's output, so `[REDACTED:*]` tags can't be re-matched by
    /// looser later patterns — no overlapping-span resolution needed.
    fn redact_blob(&self, input: &str) -> String {
        let mut s = Cow::Borrowed(input);
        if self.secrets {
            s = chain(s, &PEM_RE, "[REDACTED:private_key]");
            s = chain(s, &AWS_KEY_RE, "[REDACTED:aws_key]");
            s = chain(s, &JWT_RE, "[REDACTED:jwt]");
            s = chain(s, &BEARER_RE, "Bearer [REDACTED:token]");
            s = chain(s, &KV_SECRET_RE, "$1=[REDACTED:secret]");
        }
        if self.email {
            s = chain(s, &EMAIL_RE, "[REDACTED:email]");
        }
        if self.pan {
            s = chain(s, &PAN_RE, "[REDACTED:pan]");
        }
        if self.card {
            s = chain_validated(s, &CARD_RE, "[REDACTED:card]", |d| {
                (13..=19).contains(&d.len()) && luhn_valid(d)
            });
        }
        if self.aadhaar {
            s = chain_validated(s, &AADHAAR_RE, "[REDACTED:aadhaar]", |d| {
                d.len() == 12 && verhoeff_valid(d)
            });
        }
        if self.phone {
            s = chain(s, &PHONE_IN_RE, "[REDACTED:phone]");
            s = chain(s, &PHONE_GENERIC_RE, "[REDACTED:phone]");
        }
        if self.ip {
            s = chain_matched(s, &IPV4_RE, "[REDACTED:ip]", |m| {
                // All octets 0-255, and skip loopback/unspecified — masking
                // "listening on 127.0.0.1" hurts debugging with zero PII value.
                let octets: Vec<Option<u8>> = m.split('.').map(|o| o.parse().ok()).collect();
                octets.len() == 4
                    && octets.iter().all(|o| o.is_some())
                    && !m.starts_with("127.")
                    && m != "0.0.0.0"
            });
        }
        for (name, re) in &self.custom {
            let tag = format!("[REDACTED:{}]", name);
            s = chain(s, re, &tag);
        }
        s.into_owned()
    }
}

/// Streaming copy for `rtk proxy`: accumulates bytes until a newline, redacts
/// each complete line, writes it immediately. PII can never straddle an 8 KiB
/// read boundary because redaction only runs on whole lines. The final partial
/// line (no trailing newline) is redacted and flushed at EOF. A single line
/// growing past 1 MiB is force-flushed to bound memory (a match spanning that
/// forced flush point can be missed — pathological-only, documented).
///
/// When redaction is disabled (config or `--no-redact`) this degrades to the
/// original raw chunk copy — zero per-line overhead.
pub fn redacting_copy<R: Read, W: Write>(
    reader: R,
    writer: W,
    cap: usize,
) -> std::io::Result<Vec<u8>> {
    copy_with(global(), reader, writer, cap)
}

const MAX_LINE_BUF: usize = 1_048_576;

fn copy_with<R: Read, W: Write>(
    redactor: &Redactor,
    mut reader: R,
    mut writer: W,
    cap: usize,
) -> std::io::Result<Vec<u8>> {
    let mut captured = Vec::new();
    let mut buf = [0u8; 8192];

    let emit = |text: &[u8], captured: &mut Vec<u8>, writer: &mut W| -> std::io::Result<()> {
        if captured.len() < cap {
            let take = text.len().min(cap - captured.len());
            captured.extend_from_slice(&text[..take]);
        }
        writer.write_all(text)?;
        writer.flush()
    };

    if !redactor.enabled {
        loop {
            let count = reader.read(&mut buf)?;
            if count == 0 {
                break;
            }
            emit(&buf[..count], &mut captured, &mut writer)?;
        }
        return Ok(captured);
    }

    // Byte-level pending buffer: lossy UTF-8 conversion happens per complete
    // line, so multibyte characters split across reads stay intact.
    let mut pending: Vec<u8> = Vec::new();

    loop {
        let count = reader.read(&mut buf)?;
        if count == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..count]);

        while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=pos).collect();
            let redacted = redactor
                .redact(&String::from_utf8_lossy(&line))
                .into_owned();
            emit(redacted.as_bytes(), &mut captured, &mut writer)?;
        }

        if pending.len() > MAX_LINE_BUF {
            let redacted = redactor
                .redact(&String::from_utf8_lossy(&pending))
                .into_owned();
            pending.clear();
            emit(redacted.as_bytes(), &mut captured, &mut writer)?;
        }
    }

    if !pending.is_empty() {
        let redacted = redactor
            .redact(&String::from_utf8_lossy(&pending))
            .into_owned();
        emit(redacted.as_bytes(), &mut captured, &mut writer)?;
    }

    Ok(captured)
}

fn chain<'a>(s: Cow<'a, str>, re: &Regex, rep: &str) -> Cow<'a, str> {
    match re.replace_all(&s, rep) {
        Cow::Borrowed(_) => s,
        Cow::Owned(o) => Cow::Owned(o),
    }
}

/// Like [`chain`] but the match is only replaced when the raw match text
/// passes `valid` (octet range for IPs).
fn chain_matched<'a>(
    s: Cow<'a, str>,
    re: &Regex,
    tag: &str,
    valid: impl Fn(&str) -> bool,
) -> Cow<'a, str> {
    let mut changed = false;
    let replaced = re.replace_all(&s, |caps: &regex::Captures| {
        let m = caps.get(0).map_or("", |m| m.as_str());
        if valid(m) {
            changed = true;
            tag.to_string()
        } else {
            m.to_string()
        }
    });
    if changed {
        Cow::Owned(replaced.into_owned())
    } else {
        s
    }
}

/// Like [`chain`] but the match is only replaced when its digits pass `valid`
/// (Luhn for cards, Verhoeff for Aadhaar) — kills numeric false positives.
fn chain_validated<'a>(
    s: Cow<'a, str>,
    re: &Regex,
    tag: &str,
    valid: impl Fn(&str) -> bool,
) -> Cow<'a, str> {
    let mut changed = false;
    let replaced = re.replace_all(&s, |caps: &regex::Captures| {
        let m = caps.get(0).map_or("", |m| m.as_str());
        let digits: String = m.chars().filter(|c| c.is_ascii_digit()).collect();
        if valid(&digits) {
            changed = true;
            tag.to_string()
        } else {
            m.to_string()
        }
    });
    if changed {
        Cow::Owned(replaced.into_owned())
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        Redactor::from_config(&RedactionConfig::default(), false)
    }

    fn apply(input: &str) -> String {
        redactor().redact(input).into_owned()
    }

    /// Build a Verhoeff-valid 12-digit synthetic Aadhaar-like number by brute
    /// forcing the check digit. Never a real Aadhaar.
    fn synthetic_valid_aadhaar() -> String {
        let base = "23456789012";
        for check in 0..10 {
            let candidate = format!("{}{}", base, check);
            if verhoeff_valid(&candidate) {
                return candidate;
            }
        }
        unreachable!("one of the ten check digits must validate");
    }

    // --- positive: must redact ---

    #[test]
    fn test_redact_email() {
        let out = apply("contact ravi.chopra@slicebank.com now");
        assert!(out.contains("[REDACTED:email]"), "out={}", out);
        assert!(!out.contains("slicebank.com"), "out={}", out);
    }

    #[test]
    fn test_redact_indian_mobile_with_plus91() {
        let out = apply("call +91 9876543210 today");
        assert!(out.contains("[REDACTED:phone]"), "out={}", out);
        assert!(!out.contains("9876543210"), "out={}", out);
    }

    #[test]
    fn test_redact_indian_mobile_bare_10_digit() {
        let out = apply("mobile: 9876543210");
        assert!(out.contains("[REDACTED:phone]"), "out={}", out);
    }

    #[test]
    fn test_redact_generic_phone_with_country_code() {
        let out = apply("US office +1 415 555 0132");
        assert!(out.contains("[REDACTED:phone]"), "out={}", out);
    }

    #[test]
    fn test_redact_pan() {
        let out = apply("PAN: ABCDE1234F");
        assert!(out.contains("[REDACTED:pan]"), "out={}", out);
        assert!(!out.contains("ABCDE1234F"), "out={}", out);
    }

    #[test]
    fn test_redact_valid_aadhaar() {
        let aadhaar = synthetic_valid_aadhaar();
        let out = apply(&format!("aadhaar no {}", aadhaar));
        assert!(out.contains("[REDACTED:aadhaar]"), "out={}", out);
        assert!(!out.contains(&aadhaar), "out={}", out);
    }

    #[test]
    fn test_redact_luhn_valid_visa_16_digit() {
        // Public Visa test number.
        let out = apply("card 4111111111111111 charged");
        assert!(out.contains("[REDACTED:card]"), "out={}", out);
    }

    #[test]
    fn test_redact_luhn_valid_amex_15_digit() {
        // Public Amex test number.
        let out = apply("amex 378282246310005");
        assert!(out.contains("[REDACTED:card]"), "out={}", out);
    }

    #[test]
    fn test_redact_card_with_spaces_and_dashes() {
        let spaced = apply("pay 4111 1111 1111 1111 now");
        assert!(spaced.contains("[REDACTED:card]"), "out={}", spaced);
        let dashed = apply("pay 4111-1111-1111-1111 now");
        assert!(dashed.contains("[REDACTED:card]"), "out={}", dashed);
    }

    #[test]
    fn test_redact_aws_access_key() {
        let out = apply("key AKIAIOSFODNN7EXAMPLE leaked");
        assert!(out.contains("[REDACTED:aws_key]"), "out={}", out);
    }

    #[test]
    fn test_redact_jwt() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let out = apply(&format!("auth {}", jwt));
        assert!(out.contains("[REDACTED:jwt]"), "out={}", out);
        assert!(!out.contains("dozjgNryP4J3jVmNHl0w5N"), "out={}", out);
    }

    #[test]
    fn test_redact_bearer_token() {
        let out = apply("Authorization: Bearer abc123def456ghi789");
        assert!(out.contains("Bearer [REDACTED:token]"), "out={}", out);
        assert!(!out.contains("abc123def456ghi789"), "out={}", out);
    }

    #[test]
    fn test_redact_pem_private_key_block() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA7\nqqq\n-----END RSA PRIVATE KEY-----";
        let out = apply(&format!("dump:\n{}\ndone", pem));
        assert!(out.contains("[REDACTED:private_key]"), "out={}", out);
        assert!(!out.contains("MIIEpAIBAAKCAQEA7"), "out={}", out);
    }

    #[test]
    fn test_redact_generic_api_key_assignment() {
        let out = apply("api_key=sk_live_abcdef123456 loaded");
        assert!(out.contains("[REDACTED:secret]"), "out={}", out);
        assert!(!out.contains("sk_live_abcdef123456"), "out={}", out);
    }

    #[test]
    fn test_redact_generic_password_assignment() {
        let out = apply(r#"password: "hunter2""#);
        assert!(out.contains("[REDACTED:secret]"), "out={}", out);
        assert!(!out.contains("hunter2"), "out={}", out);
    }

    #[test]
    fn test_redact_multiple_pii_in_one_line() {
        let out = apply("ravi@slicebank.com paid with 4111111111111111 from +91 9876543210");
        assert!(out.contains("[REDACTED:email]"), "out={}", out);
        assert!(out.contains("[REDACTED:card]"), "out={}", out);
        assert!(out.contains("[REDACTED:phone]"), "out={}", out);
    }

    // --- config-driven behavior ---

    #[test]
    fn test_disabled_via_config_returns_unchanged() {
        let cfg = RedactionConfig {
            enabled: false,
            ..RedactionConfig::default()
        };
        let r = Redactor::from_config(&cfg, false);
        let input = "email ravi@slicebank.com";
        assert_eq!(r.redact(input), input);
    }

    #[test]
    fn test_no_redact_flag_overrides_enabled_config() {
        let r = Redactor::from_config(&RedactionConfig::default(), true);
        let input = "email ravi@slicebank.com";
        assert_eq!(r.redact(input), input);
    }

    #[test]
    fn test_per_category_toggle_disables_only_that_category() {
        let cfg = RedactionConfig {
            email: false,
            ..RedactionConfig::default()
        };
        let r = Redactor::from_config(&cfg, false);
        let out = r
            .redact("ravi@slicebank.com and card 4111111111111111")
            .into_owned();
        assert!(out.contains("ravi@slicebank.com"), "out={}", out);
        assert!(out.contains("[REDACTED:card]"), "out={}", out);
    }

    #[test]
    fn test_custom_pattern_from_config_is_applied() {
        let cfg = RedactionConfig {
            custom: vec![crate::core::config::CustomPattern {
                name: "employee_id".into(),
                pattern: r"EMP-\d{6}".into(),
            }],
            ..RedactionConfig::default()
        };
        let r = Redactor::from_config(&cfg, false);
        let out = r.redact("badge EMP-123456 scanned").into_owned();
        assert!(out.contains("[REDACTED:employee_id]"), "out={}", out);
        assert!(!out.contains("EMP-123456"), "out={}", out);
    }

    #[test]
    fn test_allowlist_pattern_suppresses_redaction() {
        let cfg = RedactionConfig {
            allowlist: vec!["EXAMPLE-DO-NOT-REDACT".into()],
            ..RedactionConfig::default()
        };
        let r = Redactor::from_config(&cfg, false);
        let input = "fixture@example.com EXAMPLE-DO-NOT-REDACT\nreal ravi@slicebank.com\n";
        let out = r.redact(input).into_owned();
        assert!(out.contains("fixture@example.com"), "out={}", out);
        assert!(out.contains("[REDACTED:email]"), "out={}", out);
        assert!(!out.contains("ravi@slicebank.com"), "out={}", out);
    }

    #[test]
    fn test_invalid_custom_pattern_is_skipped_not_fatal() {
        let cfg = RedactionConfig {
            custom: vec![crate::core::config::CustomPattern {
                name: "broken".into(),
                pattern: "([unclosed".into(),
            }],
            ..RedactionConfig::default()
        };
        let r = Redactor::from_config(&cfg, false);
        assert_eq!(r.redact("plain text"), "plain text");
    }

    // --- negative: must NOT redact (false-positive guards) ---

    #[test]
    fn test_git_commit_sha_not_redacted() {
        let short = "abc1234";
        let long = "34550b4d9e549c90d235051c6acc05586ac9b29e";
        let input = format!("commit {} and {}", short, long);
        assert_eq!(apply(&input), input);
    }

    #[test]
    fn test_iso_timestamp_not_redacted() {
        let input = "at 2026-07-11T12:34:56Z done";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_unix_epoch_millis_not_redacted() {
        // 13-digit epoch that fails Luhn (guarded by an assert on the test data).
        let epoch = "1770000000001";
        assert!(!luhn_valid(epoch), "pick a non-Luhn epoch for this test");
        let input = format!("ts={}", epoch);
        assert_eq!(apply(&input), input);
    }

    #[test]
    fn test_luhn_invalid_16_digit_number_not_redacted() {
        let n = "1234567890123456";
        assert!(!luhn_valid(n));
        let input = format!("job id {}", n);
        assert_eq!(apply(&input), input);
    }

    #[test]
    fn test_verhoeff_invalid_12_digit_number_not_redacted() {
        let n = "123456789012";
        assert!(!verhoeff_valid(n));
        let input = format!("ref {}", n);
        assert_eq!(apply(&input), input);
    }

    #[test]
    fn test_ipv4_address_redacted() {
        let out = apply("connection from 10.17.63.85 accepted");
        assert!(out.contains("[REDACTED:ip]"), "out={}", out);
        assert!(!out.contains("10.17.63.85"), "out={}", out);
    }

    #[test]
    fn test_ipv4_with_port_redacted_keeps_port() {
        let out = apply("listening on 192.168.1.100:8080");
        assert_eq!(out, "listening on [REDACTED:ip]:8080");
    }

    #[test]
    fn test_loopback_and_unspecified_ip_not_redacted() {
        let input = "dev server on 127.0.0.1:3000 bound to 0.0.0.0";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_invalid_octet_not_redacted_as_ip() {
        let input = "weird id 300.400.500.600 here";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_ip_category_toggle() {
        let cfg = RedactionConfig {
            ip: false,
            ..RedactionConfig::default()
        };
        let r = Redactor::from_config(&cfg, false);
        let input = "from 10.17.63.85";
        assert_eq!(r.redact(input), input);
    }

    #[test]
    fn test_redact_aws_temporary_sts_key() {
        let out = apply("AccessKeyId ASIA5F4KDTECND7YZQPX used");
        assert!(out.contains("[REDACTED:aws_key]"), "out={}", out);
        assert!(!out.contains("ASIA5F4KDTECND7YZQPX"), "out={}", out);
    }

    #[test]
    fn test_semver_not_redacted() {
        let input = "rtk v1.2.3 and lib 1.42.4 released";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_line_col_position_not_redacted() {
        let input = "error at src/main.rs:2570:12";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_uuid_not_redacted() {
        let input = "id d5efc3b9-7297-4e72-864e-7a1be53850c9";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_docker_sha256_digest_not_redacted() {
        let input = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_already_redacted_text_not_double_processed() {
        let input = "[REDACTED:email] and [REDACTED:card]";
        assert_eq!(apply(input), input);
    }

    #[test]
    fn test_clean_output_returns_borrowed_cow() {
        let r = redactor();
        let input = "Compiling rtk v0.28.2 — 3 warnings emitted";
        assert!(matches!(r.redact(input), Cow::Borrowed(_)));
    }

    #[test]
    fn test_fixture_mixed_pii_golden() {
        let raw = include_str!("../../tests/fixtures/redact_mixed_pii_raw.txt");
        let expected = include_str!("../../tests/fixtures/redact_mixed_pii_expected.txt");
        assert_eq!(apply(raw), expected);
    }

    // --- redacting_copy (proxy streaming path) ---

    /// Reader yielding fixed-size chunks — simulates PII split across reads.
    struct ChunkReader<'a> {
        data: &'a [u8],
        pos: usize,
        chunk: usize,
    }

    impl Read for ChunkReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.chunk.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn run_copy(input: &[u8], chunk: usize, enabled: bool) -> (String, Vec<u8>) {
        let cfg = RedactionConfig {
            enabled,
            ..RedactionConfig::default()
        };
        let r = Redactor::from_config(&cfg, false);
        let reader = ChunkReader {
            data: input,
            pos: 0,
            chunk,
        };
        let mut out: Vec<u8> = Vec::new();
        let captured = copy_with(&r, reader, &mut out, 1_048_576).expect("copy ok");
        (String::from_utf8_lossy(&out).into_owned(), captured)
    }

    #[test]
    fn test_redacting_copy_redacts_lines() {
        let (out, captured) = run_copy(b"hello ravi@slicebank.com\nclean line\n", 8192, true);
        assert!(out.contains("[REDACTED:email]"), "out={}", out);
        assert!(out.contains("clean line\n"), "out={}", out);
        assert!(!out.contains("ravi@slicebank.com"), "out={}", out);
        assert_eq!(captured, out.as_bytes());
    }

    #[test]
    fn test_redacting_copy_pii_split_across_chunk_boundary() {
        // 3-byte reads guarantee the email spans many read() calls.
        let (out, _) = run_copy(b"contact ravi.chopra@slicebank.com now\n", 3, true);
        assert!(out.contains("[REDACTED:email]"), "out={}", out);
        assert!(!out.contains("slicebank.com"), "out={}", out);
    }

    #[test]
    fn test_redacting_copy_partial_final_line_no_trailing_newline() {
        let (out, _) = run_copy(b"final ravi@slicebank.com", 8192, true);
        assert!(out.contains("[REDACTED:email]"), "out={}", out);
    }

    #[test]
    fn test_redacting_copy_disabled_is_raw_passthrough() {
        let input = b"raw ravi@slicebank.com stays\n";
        let (out, captured) = run_copy(input, 8192, false);
        assert_eq!(out.as_bytes(), input);
        assert_eq!(captured, input);
    }

    #[test]
    fn test_redacting_copy_no_newline_valve_bounds_memory_and_still_redacts_tail() {
        // >1 MiB single line with PII at the very end: valve force-flushes the
        // padding, EOF flush still redacts the trailing email.
        let mut input = vec![b'a'; MAX_LINE_BUF + 100];
        input.extend_from_slice(b" ravi@slicebank.com");
        let (out, _) = run_copy(&input, 8192, true);
        assert!(out.contains("[REDACTED:email]"), "tail not redacted");
        assert!(!out.contains("ravi@slicebank.com"));
    }

    // --- checksum algorithms in isolation ---

    #[test]
    fn test_luhn_valid_known_test_card_numbers() {
        // Public payment-processor test numbers, never real cards.
        assert!(luhn_valid("4111111111111111")); // Visa
        assert!(luhn_valid("5555555555554444")); // Mastercard
        assert!(luhn_valid("378282246310005")); // Amex
    }

    #[test]
    fn test_luhn_rejects_invalid_checksum() {
        assert!(!luhn_valid("4111111111111112"));
    }

    #[test]
    fn test_verhoeff_valid_known_value() {
        // Classic worked example: "236" with check digit 3.
        assert!(verhoeff_valid("2363"));
    }

    #[test]
    fn test_verhoeff_rejects_invalid_checksum() {
        assert!(!verhoeff_valid("2364"));
    }
}
