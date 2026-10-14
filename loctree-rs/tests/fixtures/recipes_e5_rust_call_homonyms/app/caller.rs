// N02 positive fixture: `analyze` is exported by BOTH diff.rs and perf.rs.
// The qualifier in the path-qualified call resolves the homonym: the edge
// must land on diff.rs and never on perf.rs.
pub fn run() {
    crate::diff::analyze();
}
