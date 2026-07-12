//! Opaque cursor tokens for paginated LSP responses (Plan 12).
//!
//! A cursor encodes `(snapshot_id, offset, kind)` so the server can
//! reject a follow-up request if the underlying snapshot has changed
//! mid-pagination (clients see `snapshot_drifted` and retry from
//! offset 0).
//!
//! Wire format: URL-safe base64 of a compact JSON object
//! `{"s": <snapshot_id>, "o": <offset>, "k": <kind>}`. Tokens are
//! opaque to clients — they MUST round-trip them verbatim.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders ⓒ 2025-2026 VetCoders

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// State carried across chunks of a paginated response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    /// Snapshot identity at the time the first chunk was issued.
    /// Server validates this on follow-up requests.
    #[serde(rename = "s")]
    pub snapshot_id: String,
    /// Offset into the result set (0-based) for the next chunk.
    #[serde(rename = "o")]
    pub offset: usize,
    /// Request method this cursor belongs to (e.g. `"loctree/find"`).
    /// Server validates that follow-ups use the same method.
    #[serde(rename = "k")]
    pub kind: String,
}

impl CursorState {
    /// Encode as opaque base64 token.
    pub fn encode(&self) -> Result<String, CursorError> {
        let json = serde_json::to_vec(self).map_err(CursorError::Encode)?;
        Ok(URL_SAFE_NO_PAD.encode(json))
    }

    /// Decode a token; validate `snapshot_id` and `kind` match the
    /// expected values. Mismatches surface as typed errors so the
    /// transport layer can return the canonical JSON-RPC error code.
    pub fn decode(
        token: &str,
        expected_snapshot: &str,
        expected_kind: &str,
    ) -> Result<Self, CursorError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(CursorError::InvalidBase64)?;
        let state: CursorState =
            serde_json::from_slice(&bytes).map_err(CursorError::InvalidJson)?;
        if state.snapshot_id != expected_snapshot {
            return Err(CursorError::SnapshotDrifted {
                expected: expected_snapshot.into(),
                got: state.snapshot_id,
            });
        }
        if state.kind != expected_kind {
            return Err(CursorError::KindMismatch {
                expected: expected_kind.into(),
                got: state.kind,
            });
        }
        Ok(state)
    }

    /// Decode without validation. Useful for inspection / tests where
    /// the caller will validate the fields by hand.
    pub fn decode_raw(token: &str) -> Result<Self, CursorError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(CursorError::InvalidBase64)?;
        serde_json::from_slice(&bytes).map_err(CursorError::InvalidJson)
    }
}

/// Errors emitted by `CursorState::decode`.
#[derive(Debug)]
pub enum CursorError {
    InvalidBase64(base64::DecodeError),
    InvalidJson(serde_json::Error),
    SnapshotDrifted { expected: String, got: String },
    KindMismatch { expected: String, got: String },
    Encode(serde_json::Error),
}

impl CursorError {
    /// Stable string code for JSON-RPC error envelopes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidBase64(_) => "cursor_invalid",
            Self::InvalidJson(_) => "cursor_invalid",
            Self::SnapshotDrifted { .. } => "snapshot_drifted",
            Self::KindMismatch { .. } => "cursor_kind_mismatch",
            Self::Encode(_) => "cursor_encode_error",
        }
    }

    /// Whether the client is expected to retry from offset 0.
    pub fn retry(&self) -> bool {
        matches!(self, Self::SnapshotDrifted { .. })
    }
}

impl std::fmt::Display for CursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBase64(err) => write!(f, "cursor token is not valid base64: {err}"),
            Self::InvalidJson(err) => write!(f, "cursor token is not valid json: {err}"),
            Self::SnapshotDrifted { expected, got } => write!(
                f,
                "snapshot drifted: cursor was issued for {got}, current snapshot is {expected} — retry from offset 0"
            ),
            Self::KindMismatch { expected, got } => write!(
                f,
                "cursor kind mismatch: token was issued by `{got}` but used with `{expected}`"
            ),
            Self::Encode(err) => write!(f, "failed to encode cursor: {err}"),
        }
    }
}

impl std::error::Error for CursorError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> CursorState {
        CursorState {
            snapshot_id: "release_1.5.0@7a64f2cb".into(),
            offset: 50,
            kind: "loctree/find".into(),
        }
    }

    #[test]
    fn roundtrip_preserves_state() {
        let original = make_state();
        let token = original.encode().expect("encode");
        let decoded =
            CursorState::decode(&token, &original.snapshot_id, &original.kind).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn token_is_url_safe() {
        let token = make_state().encode().unwrap();
        // Standard base64 produces +/=, URL-safe-no-pad uses -_ and no =.
        assert!(!token.contains('+'), "token should be URL-safe: {token}");
        assert!(!token.contains('/'), "token should be URL-safe: {token}");
        assert!(!token.contains('='), "token should be URL-safe: {token}");
    }

    #[test]
    fn snapshot_drift_is_detected() {
        let token = make_state().encode().unwrap();
        let err = CursorState::decode(&token, "different_snapshot", "loctree/find").unwrap_err();
        assert_eq!(err.code(), "snapshot_drifted");
        assert!(
            err.retry(),
            "snapshot_drifted must instruct client to retry"
        );
    }

    #[test]
    fn kind_mismatch_is_detected() {
        let token = make_state().encode().unwrap();
        let err =
            CursorState::decode(&token, "release_1.5.0@7a64f2cb", "loctree/slice").unwrap_err();
        assert_eq!(err.code(), "cursor_kind_mismatch");
        assert!(
            !err.retry(),
            "kind mismatch is a programming error, no retry"
        );
    }

    #[test]
    fn invalid_base64_is_rejected() {
        let err = CursorState::decode("!!!not-base64!!!", "x", "y").unwrap_err();
        assert_eq!(err.code(), "cursor_invalid");
    }

    #[test]
    fn invalid_json_inside_token_is_rejected() {
        let token = URL_SAFE_NO_PAD.encode(b"not json");
        let err = CursorState::decode(&token, "x", "y").unwrap_err();
        assert_eq!(err.code(), "cursor_invalid");
    }

    #[test]
    fn decode_raw_skips_validation() {
        let original = make_state();
        let token = original.encode().unwrap();
        let decoded = CursorState::decode_raw(&token).unwrap();
        assert_eq!(decoded, original);
    }
}
