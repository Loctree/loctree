pub mod alpha;

use crate::alpha::make_worker;

pub fn run_scorecard_gate() -> &'static str {
    let worker = make_worker();
    worker.render()
}
