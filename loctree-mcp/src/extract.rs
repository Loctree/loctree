use regex::Regex;
use std::sync::OnceLock;

pub fn extract_symbol(context: &str) -> &str {
    static FN_RE: OnceLock<Regex> = OnceLock::new();
    let re = FN_RE.get_or_init(|| {
        Regex::new(r"(?:async\s+)?(?:pub(?:\([^)]*\))?\s+)?fn\s+(\w+)\s*[<(]").unwrap()
    });

    if let Some(captures) = re.captures(context)
        && let Some(m) = captures.get(1)
    {
        return m.as_str();
    }

    context
        .split_whitespace()
        .filter(|t| !["{", "}", "->", "Self", "=>", "=", ":", ";", ","].contains(t))
        .rfind(|t| t.len() > 1 || t.chars().all(char::is_alphanumeric))
        .unwrap_or(context)
}
