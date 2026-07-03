//! Same struct name as config_a.rs, different shape.

pub struct QuickWinReport {
    pub severity_rank: u8,
    pub blast_radius: Vec<String>,
    pub reviewer_notes: Option<String>,
}
