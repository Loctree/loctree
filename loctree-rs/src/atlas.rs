//! Public Context Atlas API for non-CLI library consumers.
//!
//! MCP servers, LSP integrations, and editor extensions materialize and consume
//! the navigable Context Atlas (precomputed cards on disk) through this module
//! without depending on CLI-internal layout.
//!
//! For the dense in-memory ContextPack tuned for token budgets, see
//! [`crate::pack`]. The two pipelines are complementary — agents pick the shape
//! that fits.

pub use crate::cli::command::ContextOptions;
pub use crate::cli::dispatch::render_context_pack_markdown as render_context_markdown;
pub use crate::cli::dispatch::{
    ContextAtlasManifest, ContextPack, atlas_dir_for_project, compose_context_pack,
    compose_context_pack_from_snapshot, materialize_context_atlas, render_context_pack_markdown,
};
