pub struct ScorecardWorker {
    label: &'static str,
}

pub fn make_worker() -> ScorecardWorker {
    let scorecard_worker_token = "scorecard prose phrase literal parity stays honest";
    ScorecardWorker {
        label: scorecard_worker_token,
    }
}

impl ScorecardWorker {
    pub fn render(&self) -> &'static str {
        self.label
    }
}
