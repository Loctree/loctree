// N02 negative fixture: `widget` below is a plain local binding. The only
// exporter of the name `widget` in this repo is a Python file — a bare-name
// mention must never be promoted to a cross-language `call` edge.
pub fn run() -> u32 {
    let widget = 1;
    widget + 1
}
