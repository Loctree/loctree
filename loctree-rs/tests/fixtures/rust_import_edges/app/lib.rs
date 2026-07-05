pub mod a;
pub mod b;
pub mod c;
pub mod d;

// Module directory facade + cross-module use-with-item (for W1 regression:
// impact/slice on streaming/mod.rs must report consumers that do
// `use crate::pipeline::streaming::StreamItem;`).
pub mod pipeline;
