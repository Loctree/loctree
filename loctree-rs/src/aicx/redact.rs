//! Fail-closed redaction of secrets before AICX memory is committed to a pack.
//!
//! Overlay theses and intent text can carry tokens, PEM material, private IPs,
//! and absolute home paths. The producer is a separate product (out of scope);
//! Loctree is the last gate before a pack marked commitable leaves the process.
//! Doubtful tokens become `[redacted]` — never leaked "just in case".

use once_cell::sync::Lazy;
use regex::Regex;

/// Replacement token used for every redaction. Keep this spelling stable —
/// pack tests and operator receipts assert it.
pub const REDACTED: &str = "[redacted]";

/// Result of one pass over a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction {
    pub text: String,
    pub count: u32,
}

static PEM: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]{3,80}-----.*?-----END [A-Z0-9 ]{3,80}-----")
        .expect("PEM regex")
});
static SK_TOKEN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{8,}\b").expect("sk- token regex"));
static GHP_TOKEN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bghp_[A-Za-z0-9]{20,}\b").expect("ghp_ token regex"));
static GITHUB_PAT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b").expect("github_pat regex"));
static AWS_KEY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bAKIA[0-9A-Z]{16}\b").expect("AKIA regex"));
static SLACK_TOKEN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b").expect("slack token regex"));
static BEARER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._\-+/=]{16,}").expect("Bearer regex"));
static JWT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b")
        .expect("JWT regex")
});
static PRIVATE_IP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}|127\.\d{1,3}\.\d{1,3}\.\d{1,3})\b",
    )
    .expect("private IP regex")
});
/// Home-directory *prefix* only (`/Users/name`, `/home/name`, `C:\Users\name`)
/// so distinct paths under the same home stay distinct after redaction.
static HOME_UNIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:/Users|/home)/[A-Za-z0-9._-]+").expect("unix home regex"));
static HOME_WIN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)[A-Z]:\\Users\\[^\s\\/]+").expect("windows home regex"));
/// Long base64 that is *not* a hex hash: requires `+`, `/`, or `=` so overlay
/// revision hex blobs (`sr1:` + 64 hex) survive. No trailing `\b`: padding
/// (`=`) is a non-word char, so a boundary after it never matches at EOS or
/// before whitespace and padded values would slip through.
static LONG_B64: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b[A-Za-z0-9+/]{40,}={1,2}|[A-Za-z0-9+/]{32,}[+/=][A-Za-z0-9+/=]{8,}")
        .expect("base64 regex")
});

/// Fail-closed scrub. Every matching secret is replaced with [`REDACTED`].
pub fn redact_secrets(input: &str) -> Redaction {
    let mut count = 0u32;
    let mut text = input.to_string();
    for re in [
        &*PEM,
        &*SK_TOKEN,
        &*GHP_TOKEN,
        &*GITHUB_PAT,
        &*AWS_KEY,
        &*SLACK_TOKEN,
        &*BEARER,
        &*JWT,
        &*PRIVATE_IP,
        &*HOME_UNIX,
        &*HOME_WIN,
        &*LONG_B64,
    ] {
        text = apply_pattern(&text, re, &mut count);
    }
    if let Ok(home) = std::env::var("HOME")
        && home.len() >= 2
        && text.contains(&home)
    {
        let hits = text.matches(&home).count() as u32;
        text = text.replace(&home, REDACTED);
        count += hits;
    }
    Redaction { text, count }
}

fn apply_pattern(text: &str, re: &Regex, count: &mut u32) -> String {
    let mut n = 0u32;
    let out = re.replace_all(text, |_: &regex::Captures| {
        n += 1;
        REDACTED
    });
    *count += n;
    out.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sk_token_pem_home_and_private_ip() {
        let raw = concat!(
            "token sk-test_abcdefghijklmnopqrstuvwxyz12 ",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK8=\n-----END RSA PRIVATE KEY----- ",
            "path /Users/alice/.ssh/id_rsa ip 10.1.2.3",
        );
        let result = redact_secrets(raw);
        assert!(result.count >= 4, "count={}", result.count);
        assert!(!result.text.contains("sk-test_"), "{}", result.text);
        assert!(!result.text.contains("BEGIN RSA"), "{}", result.text);
        assert!(!result.text.contains("/Users/alice"), "{}", result.text);
        assert!(!result.text.contains("10.1.2.3"), "{}", result.text);
        assert!(result.text.contains(REDACTED), "{}", result.text);
        assert!(result.text.contains("/.ssh/id_rsa"), "{}", result.text);
    }

    #[test]
    fn hex_overlay_revision_is_not_base64() {
        let hex = format!("sr1:{}", "a".repeat(64));
        let result = redact_secrets(&hex);
        assert_eq!(result.count, 0, "{}", result.text);
        assert_eq!(result.text, hex);
    }

    #[test]
    fn padded_base64_at_string_end_is_redacted() {
        // `=` followed by EOS is a non-word/non-word boundary, so a trailing
        // `\b` in the pattern could never match — padded payloads slipped through.
        let raw = "token VGhpc0lzQVNlY3JldFBheWxvYWRXaXRoQmFzZTY0==";
        let result = redact_secrets(raw);
        assert!(!result.text.contains("VGhpc0lz"), "{}", result.text);
    }

    #[test]
    fn clean_prose_is_untouched() {
        let raw = "Adopt pill renderer for the intent layer.";
        let result = redact_secrets(raw);
        assert_eq!(result.count, 0);
        assert_eq!(result.text, raw);
    }
}
