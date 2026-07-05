//! Cut 5 T1 — relevance scoring + authority mapping for AICX intents.
//!
//! The `loct context --with-aicx` memory slice composer feeds in the
//! structural and runtime slices it has already built and asks this module
//! for two judgements per intent:
//!
//! 1. **Relevance** — how strongly does this intent's text overlap with the
//!    in-flight scope? We compute token overlap against a [`ScopeKeywords`]
//!    bag derived from file paths, exported symbols, and idiom-tagged
//!    runtime symbols. Empty bag → every intent gets score 1.
//!
//! 2. **Authority** — who does this intent come from? AICX intent kinds
//!    map to a small enum used by the composer to attach the right
//!    `AuthorityLabel` (`AicxOperator` / `AicxAgent` / `AicxFailure`). The
//!    enum lives here so this module never depends on the CLI handler
//!    types — the composer does the trivial cross-module mapping.
//!
//! Token overlap is intentionally simple (substring match on lower-cased
//! tokens). Embeddings-based similarity is out of scope for v1; the spec
//! flags it explicitly. The current heuristic catches the majority of
//! "agent already worked on this exact symbol/file" cases without dragging
//! in vector store dependencies.

use std::collections::HashSet;
use std::path::Path;

use super::AicxIntent;

/// Bag of lower-cased tokens harvested from the in-flight ContextPack scope.
///
/// Tokens are normalised (alphanumeric only, length ≥ 3) so that a path
/// like `loctree-rs/src/cli/dispatch/handlers/context.rs` produces tokens
/// such as `loctree`, `cli`, `dispatch`, `handlers`, `context`. CamelCase
/// and snake_case symbol names are split on case/underscore boundaries so
/// `compose_memory_slice` matches both `compose` and `memory` and `slice`.
#[derive(Debug, Clone, Default)]
pub struct ScopeKeywords {
    pub tokens: HashSet<String>,
}

impl ScopeKeywords {
    /// `true` when no scope tokens have been collected. Empty bags short-circuit
    /// scoring to score 1 — every intent is treated as equally relevant.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Number of distinct tokens currently in the bag.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Insert tokens harvested from a file path. Each path component is split
    /// on case/separator boundaries so `loctree-rs/src/lib.rs` produces
    /// `loctree`, `rs`, `src`, `lib`.
    pub fn insert_path(&mut self, path: &str) {
        for part in path.split(['/', '\\']) {
            self.add_compound_token(part);
        }
        if let Some(stem) = Path::new(path).file_stem().and_then(|s| s.to_str()) {
            self.add_compound_token(stem);
        }
    }

    /// Insert tokens harvested from a symbol name. `<file>::<symbol>` form is
    /// stripped — we want the symbol-side tokens. CamelCase / snake_case
    /// boundaries split into sub-tokens.
    pub fn insert_symbol(&mut self, symbol: &str) {
        let bare = symbol.rsplit("::").next().unwrap_or(symbol);
        self.add_compound_token(bare);
    }

    /// Add a token AND every sub-token produced by case/separator splitting.
    /// Used for path components and symbol names where a single raw chunk
    /// can carry multiple semantic words.
    fn add_compound_token(&mut self, raw: &str) {
        self.add_token(raw);
        for split in split_case_boundaries(raw) {
            self.add_token(&split);
        }
    }

    fn add_token(&mut self, raw: &str) {
        let cleaned: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();
        if cleaned.len() >= 3 {
            self.tokens.insert(cleaned);
        }
    }
}

fn split_case_boundaries(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == '.' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else if ch.is_uppercase() && !current.is_empty() {
            // CamelCase boundary — push the prior chunk before starting a new one.
            out.push(std::mem::take(&mut current));
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Authority bucket an intent belongs to before being mapped to the
/// CLI-layer `AuthorityLabel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentAuthority {
    /// Operator-stated decision/intent or operator-confirmed outcome.
    Operator,
    /// Agent-recorded task/outcome (default for non-decision kinds).
    Agent,
    /// Session marked as failed/rolled-back.
    Failure,
}

/// Explicit priority order for intent authority resolution.
///
/// Variants are intentionally ordered from highest to lowest authority. The
/// actual prioritization happens in [`resolve_authority`] via an explicit
/// match — never by if-chain ordering. Re-ordering the match arms below
/// would be visible in the diff, eliminating the cut-12 regression risk
/// where someone could silently flip semantics by reordering branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthorityResolution {
    /// Text or metadata explicitly reports failure / rollback.
    /// Highest priority — overrides kind because a rolled-back outcome is
    /// more useful flagged as a failure than as an agent outcome.
    ExplicitFailed,
    /// Kind says operator (`decision` / `intent`) or outcome carries
    /// explicit, un-negated, on-word-boundary operator tag.
    ExplicitOperator,
    /// Kind is `task` or `outcome` without operator tag.
    ExplicitAgent,
    /// Unknown kind — heuristic fallback to agent.
    HeuristicGuess,
}

impl AuthorityResolution {
    /// Map the explicit resolution to the consumer-facing
    /// [`IntentAuthority`] bucket. The mapping is total and stable — every
    /// resolution must terminate in a known bucket so the composer never
    /// sees an unclassified intent.
    pub fn into_authority(self) -> IntentAuthority {
        match self {
            Self::ExplicitFailed => IntentAuthority::Failure,
            Self::ExplicitOperator => IntentAuthority::Operator,
            Self::ExplicitAgent | Self::HeuristicGuess => IntentAuthority::Agent,
        }
    }
}

/// Score a single intent against the active scope keyword bag.
///
/// Lowercase substring overlap: each scope token that appears anywhere in
/// the intent text earns one point. An empty keyword bag yields a flat
/// score of `1` — useful for the project-wide `--with-aicx` mode where
/// every recent decision is potentially relevant.
pub fn score_intent(intent: &AicxIntent, keywords: &ScopeKeywords) -> u32 {
    if keywords.is_empty() {
        return 1;
    }
    let lowered = intent.text.to_lowercase();
    keywords
        .tokens
        .iter()
        .filter(|kw| lowered.contains(kw.as_str()))
        .count() as u32
}

/// Resolve the intent's full [`AuthorityResolution`] — the explicit-priority
/// classification used by [`authority_for_intent`].
///
/// Decision order (encoded as a `match` on the resolution enum, not as an
/// if-chain — see Plan L01 / Findings #2 #3 #4 / Monika 2026-05-06):
///
/// 1. [`AuthorityResolution::ExplicitFailed`] — text or metadata
///    explicitly reports failure / rollback (after word-boundary,
///    negation, conditional, and metadata-suffix guards).
/// 2. [`AuthorityResolution::ExplicitOperator`] — kind is
///    `decision` / `intent`, or kind is `outcome` and the text carries an
///    un-negated operator tag.
/// 3. [`AuthorityResolution::ExplicitAgent`] — kind is `task`, or kind is
///    `outcome` without operator tag.
/// 4. [`AuthorityResolution::HeuristicGuess`] — unknown kind.
pub fn resolve_authority(intent: &AicxIntent) -> AuthorityResolution {
    if intent_is_failure(&intent.text) {
        return AuthorityResolution::ExplicitFailed;
    }
    match intent.kind.to_lowercase().as_str() {
        "decision" | "intent" => AuthorityResolution::ExplicitOperator,
        "outcome" => {
            if outcome_is_operator_tagged(&intent.text) {
                AuthorityResolution::ExplicitOperator
            } else {
                AuthorityResolution::ExplicitAgent
            }
        }
        "task" => AuthorityResolution::ExplicitAgent,
        _ => AuthorityResolution::HeuristicGuess,
    }
}

/// Resolve the intent's authority bucket per Cut 5 T1 spec.
///
/// Thin wrapper over [`resolve_authority`] that flattens to the
/// consumer-facing [`IntentAuthority`] enum. Existing call sites in
/// `pack.rs` and `cli/dispatch/handlers/context/mod.rs` keep their
/// signature; the priority is explicit in the enum, not in if-chain order.
pub fn authority_for_intent(intent: &AicxIntent) -> IntentAuthority {
    resolve_authority(intent).into_authority()
}

/// Phrases that, when found unguarded and on word boundaries, mark a session
/// as failed. Order is irrelevant — any match wins.
const FAILURE_PHRASES: &[&str] = &[
    "failed",
    "failure",
    "rolled back",
    "rollback",
    "regression",
    "broken build",
];

/// Phrases that promote an `outcome` intent from agent-authored to
/// operator-confirmed. Order is irrelevant — any match wins.
const OPERATOR_PHRASES: &[&str] = &[
    "operator decision",
    "operator confirmed",
    "operator approved",
    "merged to develop",
    "merged to main",
    "shipped",
];

fn intent_is_failure(text: &str) -> bool {
    let lowered = text.to_lowercase();
    if has_explicit_failure_metadata(&lowered) {
        return true;
    }
    FAILURE_PHRASES
        .iter()
        .any(|marker| contains_asserted_failure_marker(&lowered, marker))
}

fn outcome_is_operator_tagged(text: &str) -> bool {
    let lowered = text.to_lowercase();
    if has_explicit_operator_metadata(&lowered) {
        return true;
    }
    OPERATOR_PHRASES
        .iter()
        .any(|marker| contains_affirmed_operator_marker(&lowered, marker))
}

/// Detect explicit failure metadata that overrides the heuristic phrase
/// matcher. Strongest signal — when a session text records `status:
/// failed`, the agent or operator has already done the classification for
/// us; we should not re-evaluate the surrounding prose.
fn has_explicit_failure_metadata(lowered: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "status: failed",
        "status:failed",
        "status: rolled_back",
        "status:rolled_back",
        "status: rolled back",
        "result: failure",
        "result:failure",
        "outcome: failed",
        "outcome:failed",
        "rolled_back: true",
        "failed: true",
    ];
    PATTERNS.iter().any(|p| lowered.contains(p))
}

/// Detect explicit operator-confirmed metadata. Symmetric counterpart of
/// [`has_explicit_failure_metadata`] for the `outcome` → operator promotion
/// path. We deliberately keep this list short — operator confirmation is
/// usually expressed in prose, not in metadata key-value pairs.
fn has_explicit_operator_metadata(lowered: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "operator: confirmed",
        "operator:confirmed",
        "operator_confirmed: true",
        "shipped: true",
        "merged: true",
    ];
    PATTERNS.iter().any(|p| lowered.contains(p))
}

/// Look for the first phrase in `phrases` that fires unguarded inside
/// `haystack` and sits on word boundaries (so substrings inside compound
/// identifiers like `regression-failure-mode-hint` are rejected).
///
/// Negation / conditional / suffix-metadata markers are supplied via
/// `prefix_guards` and `suffix_guards`; the predicate
/// [`marker_is_guarded`] decides whether a positional hit is muted.
///
/// Returns `Some(matched_phrase)` for an unambiguous, on-word-boundary,
/// un-guarded hit; `None` otherwise. Currently used internally by
/// [`intent_is_failure`] and [`outcome_is_operator_tagged`] via
/// [`contains_asserted_failure_marker`] / [`contains_affirmed_operator_marker`].
pub fn classify_phrase_hit<'a>(
    haystack: &str,
    phrases: &'a [&'a str],
    prefix_guards: &[&str],
    suffix_guards: &[&str],
) -> Option<&'a str> {
    let lowered = haystack.to_lowercase();
    for marker in phrases {
        let any_clean_hit = lowered.match_indices(marker).any(|(idx, _)| {
            !marker_is_guarded(&lowered, idx, marker.len(), prefix_guards, suffix_guards)
        });
        if any_clean_hit {
            return Some(*marker);
        }
    }
    None
}

fn contains_asserted_failure_marker(lowered: &str, marker: &str) -> bool {
    lowered
        .match_indices(marker)
        .any(|(idx, _)| !failure_marker_is_guarded(lowered, idx, marker.len()))
}

fn contains_affirmed_operator_marker(lowered: &str, marker: &str) -> bool {
    lowered
        .match_indices(marker)
        .any(|(idx, _)| !operator_marker_is_guarded(lowered, idx, marker.len()))
}

fn failure_marker_is_guarded(lowered: &str, start: usize, marker_len: usize) -> bool {
    const PREFIX_GUARDS: &[&str] = &[
        "not ",
        "no ",
        "never ",
        "without ",
        "avoid ",
        "avoids ",
        "avoiding ",
        "prevent ",
        "prevents ",
        "preventing ",
        "don't ",
        "don't introduce ",
        "don't introduce any ",
        "do not ",
        "do not introduce ",
        "do not introduce any ",
        "should not ",
        "should not introduce ",
        "must not ",
        "must not introduce ",
        "if ",
        "when ",
        "unless ",
        "until ",
        "risk of ",
        "possible ",
        "potential ",
    ];
    const SUFFIX_GUARDS: &[&str] = &[
        "?",
        ": false",
        " false",
        " risk",
        " risks",
        " test",
        " tests",
        " suite",
        " guard",
        " guards",
        " prevention",
        " budget",
    ];

    marker_is_guarded(lowered, start, marker_len, PREFIX_GUARDS, SUFFIX_GUARDS)
}

fn operator_marker_is_guarded(lowered: &str, start: usize, marker_len: usize) -> bool {
    const PREFIX_GUARDS: &[&str] = &[
        "not ",
        "no ",
        "never ",
        "pending ",
        "should be ",
        "should not be ",
        "should not ",
        "must not be ",
        "must not ",
        "if ",
        "when ",
        "unless ",
        "until ",
        "only when ",
    ];
    const SUFFIX_GUARDS: &[&str] = &[": false", "?", "pending", "still pending", "not yet"];

    marker_is_guarded(lowered, start, marker_len, PREFIX_GUARDS, SUFFIX_GUARDS)
}

fn marker_is_guarded(
    lowered: &str,
    start: usize,
    marker_len: usize,
    prefix_guards: &[&str],
    suffix_guards: &[&str],
) -> bool {
    if !on_word_boundaries(lowered, start, marker_len) {
        return true;
    }

    let prefix = &lowered[..start];
    let prefix_tail_start = prefix
        .char_indices()
        .rev()
        .nth(40)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let prefix_tail = &prefix[prefix_tail_start..];
    if prefix_guards
        .iter()
        .any(|guard| prefix_tail.trim_end().ends_with(guard.trim_end()))
        || prefix_tail.contains("if ")
        || prefix_tail.contains("when ")
        || prefix_tail.contains("unless ")
        || prefix_tail.contains("until ")
    {
        return true;
    }

    let suffix = &lowered[start + marker_len..];
    suffix_guards
        .iter()
        .any(|guard| suffix.trim_start().starts_with(guard.trim_start()))
}

/// `true` when the substring `[start, start+marker_len)` of `s` is bounded
/// by non-word characters on both sides — preventing substring hits inside
/// compound technical identifiers like `regression-failure-mode-hint` or
/// `failure_mode_analysis`.
///
/// `_` and `-` count as word characters because Rust / shell / kebab-case
/// identifiers join words with them and a marker hit inside such an
/// identifier is almost always a substring artefact, not a real failure
/// report.
fn on_word_boundaries(s: &str, start: usize, marker_len: usize) -> bool {
    let before_ok = if start == 0 {
        true
    } else {
        match s[..start].chars().next_back() {
            Some(ch) => !is_identifier_char(ch),
            None => true,
        }
    };
    let after_idx = start + marker_len;
    let after_ok = if after_idx >= s.len() {
        true
    } else {
        match s[after_idx..].chars().next() {
            Some(ch) => !is_identifier_char(ch),
            None => true,
        }
    };
    before_ok && after_ok
}

#[inline]
fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_intent(kind: &str, text: &str) -> AicxIntent {
        AicxIntent {
            kind: kind.to_string(),
            text: text.to_string(),
            agent: "claude".to_string(),
            date: "2026-04-28".to_string(),
            timestamp: Some("2026-04-28T01:43:37Z".to_string()),
            session_id: "session-test".to_string(),
            project: "loctree-suite".to_string(),
            source_chunk_path: "/tmp/aicx/store/test.md".to_string(),
            frame_kind: None,
            oracle_status: None,
        }
    }

    #[test]
    fn scope_keywords_extract_path_components() {
        let mut bag = ScopeKeywords::default();
        bag.insert_path("loctree-rs/src/cli/dispatch/handlers/context.rs");
        assert!(bag.tokens.contains("loctree"));
        assert!(bag.tokens.contains("cli"));
        assert!(bag.tokens.contains("dispatch"));
        assert!(bag.tokens.contains("handlers"));
        assert!(bag.tokens.contains("context"));
    }

    #[test]
    fn scope_keywords_split_case_boundaries() {
        let mut bag = ScopeKeywords::default();
        bag.insert_symbol("ComposeMemorySlice");
        assert!(bag.tokens.contains("compose"));
        assert!(bag.tokens.contains("memory"));
        assert!(bag.tokens.contains("slice"));

        let mut bag = ScopeKeywords::default();
        bag.insert_symbol("compose_memory_slice");
        assert!(bag.tokens.contains("compose"));
        assert!(bag.tokens.contains("memory"));
        assert!(bag.tokens.contains("slice"));
    }

    #[test]
    fn scope_keywords_strip_file_prefix_in_symbol_id() {
        let mut bag = ScopeKeywords::default();
        bag.insert_symbol("loctree-rs/src/lib.rs::run_context");
        assert!(bag.tokens.contains("run"));
        assert!(bag.tokens.contains("context"));
    }

    #[test]
    fn scope_keywords_drop_short_tokens() {
        let mut bag = ScopeKeywords::default();
        bag.insert_symbol("a");
        bag.insert_symbol("ab");
        bag.insert_symbol("abc");
        assert!(!bag.tokens.contains("a"));
        assert!(!bag.tokens.contains("ab"));
        assert!(bag.tokens.contains("abc"));
    }

    #[test]
    fn score_intent_zero_when_no_overlap() {
        let mut bag = ScopeKeywords::default();
        bag.insert_path("src/payments/stripe.rs");
        let intent = make_intent(
            "decision",
            "Refactored auth middleware to drop legacy tokens",
        );
        assert_eq!(score_intent(&intent, &bag), 0);
    }

    #[test]
    fn score_intent_counts_overlapping_tokens() {
        let mut bag = ScopeKeywords::default();
        bag.insert_path("loctree-rs/src/cli/dispatch/handlers/context.rs");
        bag.insert_symbol("compose_memory_slice");
        let intent = make_intent(
            "decision",
            "Cut 5 T1: introduce compose_memory_slice in the context handler",
        );
        let score = score_intent(&intent, &bag);
        assert!(score >= 3, "expected at least 3 overlaps, got {score}");
    }

    #[test]
    fn score_intent_returns_one_for_empty_bag() {
        let bag = ScopeKeywords::default();
        let intent = make_intent("intent", "Something completely unrelated");
        assert_eq!(score_intent(&intent, &bag), 1);
    }

    #[test]
    fn authority_for_intent_decision_is_operator() {
        let intent = make_intent("decision", "Adopt cargo workspaces for the LSP crate");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Operator);
    }

    #[test]
    fn authority_for_intent_intent_kind_is_operator() {
        let intent = make_intent("intent", "Add memory slice composer");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Operator);
    }

    #[test]
    fn authority_for_intent_outcome_default_is_agent() {
        let intent = make_intent("outcome", "Tests pass on macOS and Linux");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Agent);
    }

    #[test]
    fn authority_for_intent_outcome_promoted_when_operator_tagged() {
        let intent = make_intent("outcome", "Operator confirmed merged to develop");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Operator);
    }

    #[test]
    fn authority_for_intent_outcome_does_not_promote_guarded_operator_tags() {
        for text in [
            "We have not shipped yet",
            "Feature should be shipped only when tests pass",
            "shipped: false",
            "This should not be merged to develop until review",
            "If X is merged to main then release is risky",
            "operator approved? still pending",
        ] {
            let intent = make_intent("outcome", text);
            assert_eq!(
                authority_for_intent(&intent),
                IntentAuthority::Agent,
                "{text}"
            );
        }
    }

    #[test]
    fn authority_for_intent_task_is_agent() {
        let intent = make_intent("task", "Write five unit tests for compose_memory_slice");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Agent);
    }

    #[test]
    fn authority_for_intent_failure_overrides_kind() {
        let intent = make_intent(
            "outcome",
            "Operator confirmed shipped: but the build failed on Windows",
        );
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Failure);
    }

    #[test]
    fn authority_for_intent_failure_does_not_promote_guarded_markers() {
        for text in [
            "Don't introduce any regression in the cache path",
            "Do not introduce regression while cleaning transcripts",
            "No regression in the auth flow",
            "Potential regression risk if this is merged",
            "Regression tests cover the parser",
            "failed: false",
            "Failure? still pending investigation",
        ] {
            let intent = make_intent("outcome", text);
            assert_eq!(
                authority_for_intent(&intent),
                IntentAuthority::Agent,
                "{text}"
            );
        }
    }

    #[test]
    fn authority_for_intent_failure_promotes_asserted_markers() {
        for text in [
            "Codex introduced a regression in transcript cleanup",
            "The release failed on Windows",
            "The feature rolled back after smoke tests",
            "Broken build after merge",
        ] {
            let intent = make_intent("outcome", text);
            assert_eq!(
                authority_for_intent(&intent),
                IntentAuthority::Failure,
                "{text}"
            );
        }
    }

    #[test]
    fn authority_for_intent_unknown_kind_falls_back_to_agent() {
        let intent = make_intent("note", "Random side note");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Agent);
    }

    // -----------------------------------------------------------------
    // L01 / Findings #2 #3 #4 — word-boundary, unless-guard,
    // explicit-metadata and AuthorityResolution priority tests.
    // -----------------------------------------------------------------

    #[test]
    fn intent_is_failure_rejects_prevention_guidance() {
        // Verbatim audit example from Monika 2026-05-06.
        let intent = make_intent(
            "intent",
            "Do not modify source files unless smoke test reveals a regression.",
        );
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Operator);
    }

    #[test]
    fn intent_is_failure_rejects_substring_inside_identifier() {
        // `failure` substring inside compound identifier — not a failure
        // report. Word-boundary guard with `-` as identifier char.
        for text in [
            "Tracking the regression-failure-mode-hint test fixture",
            "Touched the failure_mode_analysis subsystem",
            "Updated regression-test-suite docs",
        ] {
            let intent = make_intent("outcome", text);
            assert_eq!(
                authority_for_intent(&intent),
                IntentAuthority::Agent,
                "{text}"
            );
        }
    }

    #[test]
    fn intent_is_failure_accepts_explicit_failed_status() {
        for text in [
            "status: failed — auth middleware rolled back overnight",
            "outcome: failed",
            "result: failure during stripe handshake",
            "rolled_back: true on prod-eu",
        ] {
            let intent = make_intent("outcome", text);
            assert_eq!(
                authority_for_intent(&intent),
                IntentAuthority::Failure,
                "{text}"
            );
        }
    }

    #[test]
    fn outcome_is_operator_tagged_rejects_not_shipped_yet() {
        let intent = make_intent("outcome", "We have not shipped yet");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Agent);
    }

    #[test]
    fn outcome_is_operator_tagged_rejects_should_not_be_merged() {
        let intent = make_intent("outcome", "should not be merged to main until tests pass");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Agent);
    }

    #[test]
    fn outcome_is_operator_tagged_rejects_shipped_false_metadata() {
        let intent = make_intent("outcome", "shipped: false");
        assert_eq!(authority_for_intent(&intent), IntentAuthority::Agent);
    }

    #[test]
    fn outcome_is_operator_tagged_accepts_explicit_metadata() {
        for text in [
            "operator: confirmed",
            "shipped: true to production at 14:00",
            "merged: true after gate review",
        ] {
            let intent = make_intent("outcome", text);
            assert_eq!(
                authority_for_intent(&intent),
                IntentAuthority::Operator,
                "{text}"
            );
        }
    }

    #[test]
    fn authority_resolution_priority_order_is_explicit() {
        // Failure overrides operator-confirmed prose — encoded in match
        // arm order of `resolve_authority`, not in if-chain ordering.
        let failure = make_intent(
            "outcome",
            "operator confirmed shipped: but the build failed on Windows",
        );
        assert_eq!(
            resolve_authority(&failure),
            AuthorityResolution::ExplicitFailed
        );
        assert_eq!(failure_resolution_to_authority(), IntentAuthority::Failure);

        let operator = make_intent("decision", "Adopt cargo workspaces for the LSP crate");
        assert_eq!(
            resolve_authority(&operator),
            AuthorityResolution::ExplicitOperator
        );

        let agent = make_intent("task", "Write five unit tests for compose_memory_slice");
        assert_eq!(
            resolve_authority(&agent),
            AuthorityResolution::ExplicitAgent
        );

        let unknown = make_intent("note", "Random side note");
        assert_eq!(
            resolve_authority(&unknown),
            AuthorityResolution::HeuristicGuess
        );
        assert_eq!(unknown_heuristic_to_authority(), IntentAuthority::Agent);
    }

    fn failure_resolution_to_authority() -> IntentAuthority {
        AuthorityResolution::ExplicitFailed.into_authority()
    }

    fn unknown_heuristic_to_authority() -> IntentAuthority {
        AuthorityResolution::HeuristicGuess.into_authority()
    }

    #[test]
    fn classify_phrase_hit_returns_marker_when_unguarded() {
        let prefix_guards: &[&str] = &["not ", "do not "];
        let suffix_guards: &[&str] = &[": false", "?"];
        let phrases = &["shipped", "merged to main"];
        assert_eq!(
            classify_phrase_hit(
                "We shipped to production today",
                phrases,
                prefix_guards,
                suffix_guards
            ),
            Some("shipped")
        );
    }

    #[test]
    fn classify_phrase_hit_returns_none_when_negated() {
        let prefix_guards: &[&str] = &["not ", "do not "];
        let suffix_guards: &[&str] = &[": false", "?"];
        let phrases = &["shipped", "merged to main"];
        assert_eq!(
            classify_phrase_hit(
                "We have not shipped yet",
                phrases,
                prefix_guards,
                suffix_guards
            ),
            None
        );
    }

    #[test]
    fn classify_phrase_hit_returns_none_for_substring_inside_identifier() {
        let prefix_guards: &[&str] = &["not "];
        let suffix_guards: &[&str] = &[": false"];
        let phrases = &["failure"];
        assert_eq!(
            classify_phrase_hit(
                "regression-failure-mode-hint fixture name",
                phrases,
                prefix_guards,
                suffix_guards
            ),
            None
        );
    }
}
