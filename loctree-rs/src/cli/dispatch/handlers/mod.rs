//! Command handlers split by domain
//!
//! This module organizes command handlers into domain-specific submodules
//! to keep the codebase maintainable.

pub mod ai;
pub mod analysis;
pub mod anchors;
pub mod cache;
pub mod context;
pub mod deprecation;
pub mod diff;
pub mod doctor;
pub mod env_truth;
pub mod history;
pub mod inventory;
pub mod occurrences;
pub mod output;
pub mod prism;
pub mod prune;
pub mod query;
pub mod resource_limits;
pub mod suppressions;
pub mod watch;
