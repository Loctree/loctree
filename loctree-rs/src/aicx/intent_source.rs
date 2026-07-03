//! Forward-compat seam for the eventual `aicx` library crate.
//!
//! # Why this trait exists
//!
//! Loctree currently consumes the AICX memory store through a CLI
//! subprocess (and optionally `aicx-mcp` over stdio JSON-RPC). Both
//! paths are encapsulated by [`crate::aicx::AicxClient`].
//!
//! The AICX team is mid-flight on shipping `aicx-retrieve` as a
//! consumable Cargo crate (`aicx-retrieve::OracleContract` +
//! `aicx-retrieve::HybridIndex` are the planned public surface).
//! Once that lands, loctree will swap the subprocess wrapper for an
//! in-process library handle and erase one process-spawn per query.
//!
//! Wave 6b (actual library linking) is BLOCKED on AICX shipping
//! `aicx = { features = ["canonical-store"] }` with a stable trait
//! shape. Until then, loctree carries this trait as a
//! *structurally-identical* mirror of the eventual
//! `aicx::IntentStore` so the eventual swap is a one-line `use`
//! change: `pub use aicx::IntentStore as IntentSource;` plus
//! deleting the [`crate::aicx::AicxClient`] impl block.
//!
//! # Reference shape (from the AICX action plan)
//!
//! ```ignore
//! // aicx side, eventual:
//! pub trait IntentStore: Send + Sync {
//!     fn intents(&self, query: IntentsQuery)        -> Result<IntentsResponse, RetrieveError>;
//!     fn steer(&self, filters: SteerFilters)        -> Result<SteerResponse,   RetrieveError>;
//!     fn search_literal(&self, query: &str, opts: SearchOptions)
//!                                                   -> Result<SearchResponse,  RetrieveError>;
//!     fn read(&self, source_chunk: &Path)           -> Result<String,          RetrieveError>;
//!     fn oracle(&self)                              -> OracleContract;
//! }
//! ```
//!
//! Loctree-side today uses simpler signatures because:
//!
//! - `IntentsResponse`/`SearchResponse`/`SteerResponse` envelope types
//!   do not exist yet on the AICX side. Loctree returns the bare
//!   `Vec<…>` rows it already consumes; per-row `oracle_status` is
//!   the operative provenance hook.
//! - `RetrieveError` does not exist yet either. The current contract
//!   is "empty on failure" (see [`crate::aicx::AicxClient`] doc) —
//!   the library will tighten this later.
//! - `read(source_chunk)` and `oracle()` are deliberately omitted —
//!   loctree consumers do not need either today. They will be added
//!   when Wave 6b actually links the library.
//!
//! # Why a trait instead of a concrete swap-out
//!
//! Holding the trait gives future code a single seam for:
//!
//! - Fakes in unit tests (today: shell-script mocks per test;
//!   tomorrow: a `MockIntentSource` that hands back canned rows
//!   without touching env vars).
//! - The blocker doc cited at
//!   `~/internal-artifacts/inbox/Loctree/aicx/blockers/loctree-side-needs.md`.
//! - The eventual paid-tier vs. free-tier split (`aicx = { features =
//!   ["canonical-store"] }` for free; add `"semantic"` for the
//!   embedding/LLM-enriched paid build). Loctree's trait surface does
//!   not change; only the impl behind it does.
//!
//! # Today's impl
//!
//! [`CliIntentSource`] is the type alias for the only impl that ships
//! today — [`crate::aicx::AicxClient`]. Existing call sites that hold
//! `&AicxClient` are unaffected; new call sites should prefer
//! `&dyn IntentSource` so they swap cleanly when Wave 6b lands.

use super::{AicxClient, AicxIntent, AicxSearchResult, AicxSteerResult, SteerFilters};

/// Read-only intent / steer / search retrieval surface.
///
/// Structurally mirrors the eventual `aicx::IntentStore` trait so
/// that wiring up Wave 6b is a one-line `use` swap rather than a
/// signature migration. See the module-level docs for the full
/// rationale and the divergence list.
///
/// # Contract
///
/// All methods are **read-only** and **failure-soft**: on transport
/// failure the impl returns an empty `Vec` rather than propagating
/// the error. This mirrors today's [`AicxClient`] contract — agents
/// composing context must not be killed by an AICX hiccup. The
/// per-row [`super::OracleStatus`] still carries provenance so
/// callers know whether a row came from the semantic oracle or a
/// fuzzy fallback (see [`super::OracleStatus::readiness`]).
///
/// # Implementor obligations
///
/// - `Send + Sync` so a single instance can be shared across the
///   composition pipeline by `&` reference.
/// - Cache hygiene is implementor-side. The today-impl
///   ([`AicxClient`]) caches successful empty results but never
///   caches failures (see Plan L03 / Finding #8).
/// - Per-row `oracle_status` MUST be populated whenever the wire
///   transport delivers it. Consumers gate scope-narrowing decisions
///   on `oracle_status.loctree_scope_safe`; dropping the field
///   silently promotes fuzzy-fallback rows to oracle authority.
pub trait IntentSource: Send + Sync {
    /// Fetch structured intents for the given window.
    ///
    /// `window_hours` bounds the extraction window in hours.
    /// `limit` caps the number of returned rows. Returns an empty
    /// `Vec` whenever the underlying transport cannot serve a
    /// result (binary missing, MCP timeout in soft mode, project
    /// scope unsupported by the surface, etc.).
    fn intents(&self, window_hours: u64, limit: usize) -> Vec<AicxIntent>;

    /// Steer-filter chunks by frontmatter metadata.
    ///
    /// Returns an empty `Vec` on any failure. The free-tier surface
    /// does not enrich the rows beyond what AICX persists in chunk
    /// frontmatter; the paid-tier (semantic) build may add
    /// embedding-derived ranking once Wave 6b lands.
    fn steer(&self, filters: SteerFilters) -> Vec<AicxSteerResult>;

    /// Search the canonical AICX corpus.
    ///
    /// `hours` bounds the time window (0 = all time) where the
    /// active transport supports it. `limit` caps the returned set.
    /// Returns an empty `Vec` on any failure. Per-row
    /// [`super::OracleStatus`] tells callers whether the answer came
    /// from the semantic oracle or a degraded layer; gate
    /// scope-narrowing on [`super::OracleStatus::readiness`], not on
    /// the mere non-emptiness of the result.
    fn search(&self, query: &str, hours: u64, limit: usize) -> Vec<AicxSearchResult>;
}

/// Type alias for the only [`IntentSource`] impl that ships today —
/// the subprocess-backed [`AicxClient`].
///
/// Held as an alias rather than a `pub use` rename so the existing
/// `AicxClient` type stays the documented entry point for direct
/// construction (the trait is the consumption surface; the alias is
/// the documentation handle for "this is the CLI-backed impl").
///
/// When Wave 6b lands a second impl (`LibIntentSource` over the
/// in-process `aicx-retrieve::HybridIndex`), this alias stays
/// pointing at the CLI variant so legacy operator workflows that
/// rely on the subprocess still resolve.
pub type CliIntentSource = AicxClient;

impl IntentSource for AicxClient {
    fn intents(&self, window_hours: u64, limit: usize) -> Vec<AicxIntent> {
        AicxClient::intents(self, window_hours, limit)
    }

    fn steer(&self, filters: SteerFilters) -> Vec<AicxSteerResult> {
        AicxClient::steer(self, filters)
    }

    fn search(&self, query: &str, hours: u64, limit: usize) -> Vec<AicxSearchResult> {
        AicxClient::search(self, query, hours, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// In-memory fake used by `pack::tests` to exercise the trait
    /// without spawning the AICX subprocess. Closes the test gap
    /// from Wave 6b prep: the legacy tests had to write shell-script
    /// mocks to a tempdir; a trait-shaped fake collapses that to
    /// "construct, hand back canned rows".
    pub(crate) struct CannedIntentSource {
        intents: Mutex<Vec<AicxIntent>>,
    }

    impl CannedIntentSource {
        pub(crate) fn new(intents: Vec<AicxIntent>) -> Self {
            Self {
                intents: Mutex::new(intents),
            }
        }
    }

    impl IntentSource for CannedIntentSource {
        fn intents(&self, _window_hours: u64, _limit: usize) -> Vec<AicxIntent> {
            self.intents.lock().map(|v| v.clone()).unwrap_or_default()
        }

        fn steer(&self, _filters: SteerFilters) -> Vec<AicxSteerResult> {
            Vec::new()
        }

        fn search(&self, _query: &str, _hours: u64, _limit: usize) -> Vec<AicxSearchResult> {
            Vec::new()
        }
    }

    #[test]
    fn canned_intent_source_returns_canned_rows() {
        let intent = AicxIntent {
            kind: "decision".to_string(),
            text: "trait smoke test".to_string(),
            agent: "claude".to_string(),
            date: "2026-05-17".to_string(),
            timestamp: Some("2026-05-17T10:00:00Z".to_string()),
            session_id: "s0".to_string(),
            project: "loctree-suite".to_string(),
            source_chunk_path: "/tmp/aicx/s0.md".to_string(),
            frame_kind: None,
            oracle_status: None,
        };
        let src = CannedIntentSource::new(vec![intent.clone()]);
        let got = src.intents(168, 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, intent.text);
        // Steer/search are empty stubs.
        assert!(src.steer(SteerFilters::default()).is_empty());
        assert!(src.search("anything", 0, 10).is_empty());
    }

    #[test]
    fn aicx_client_impls_intent_source_trait() {
        // Compile-time witness: the trait object is constructible
        // from a real `AicxClient`. Runtime behaviour exercised by
        // the existing `aicx::tests` module — this only proves the
        // trait surface stays in lockstep with the concrete type.
        fn assert_impl<T: IntentSource + ?Sized>(_: &T) {}

        // Use the test-mode kill switch so this never spawns a
        // subprocess even if the harness has `LOCT_AICX_BINARY` set.
        let client = AicxClient::new("loctree-suite");
        let as_trait: &dyn IntentSource = &client;
        assert_impl(as_trait);
    }
}
