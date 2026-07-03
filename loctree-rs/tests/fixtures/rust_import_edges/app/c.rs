pub use crate::a::Foo;

/// Cross-module use with trailing item on a module-dir facade (W1 regression).
/// Ensures `use crate::pipeline::streaming::StreamItem;` wires a consumer edge
/// to the facade mod.rs so impact/slice report the true importers.
use crate::pipeline::streaming::StreamItem;
