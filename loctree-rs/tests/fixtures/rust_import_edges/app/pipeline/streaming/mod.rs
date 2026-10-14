// Leaf module via directory (streaming/mod.rs). This is the target for
// facade + cross-module `use crate::...::streaming::Item` consumer edges.
// See loctree-feedback.md:2997 and the W1 items in dispatch brief.
pub struct StreamItem;

pub fn make_stream() -> StreamItem {
    StreamItem
}
