//! F1 fixture: a dead fn in `hf_cache.rs` plus an UNRELATED string literal
//! elsewhere that happens to contain the file's stem ("hf_cache"), with no
//! spawn/exec call anywhere near it. The literal cross-check must not treat
//! this as spawn-by-string evidence — the candidate stays high-confidence.

pub mod hf_cache;

pub fn log_startup() {
    println!("loading provider from hf_cache directory");
}
