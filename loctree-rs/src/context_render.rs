//! Shared ContextPack rendering helpers.
//!
//! Keep these outside CLI handler modules so library-layer composition never
//! needs to import from `crate::cli::*`.

/// Canonical advertised command for opaque chunk pointers. `aicx open` is an
/// alias on the producer CLI; packs and pills must spell `aicx read`.
pub(crate) fn aicx_read_chunk_footer(n: usize) -> String {
    format!(
        "_{n} unique chunk(s) reachable via `aicx read <chunk:ref>` (resolved against the operator's local aicx store; absolute paths intentionally redacted to keep this context-pack commitable)._"
    )
}

/// Render an opaque chunk reference for a source_chunk path.
///
/// The hash is `sha256(source_chunk_path)[..8]` hex. Empty input maps to the
/// literal `chunk:none` so renderers never panic on missing provenance.
pub(crate) fn chunk_ref(source_chunk: &str) -> String {
    use sha2::{Digest, Sha256};

    if source_chunk.is_empty() {
        return "chunk:none".to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(source_chunk.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("chunk:{hex}")
}

pub(crate) fn current_iso_timestamp() -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
