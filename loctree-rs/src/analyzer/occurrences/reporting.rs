use std::collections::HashMap;

use super::{FileCount, OccurrencePage, OccurrenceResults, ReportOptions, ScopeClassification};

impl OccurrenceResults {
    /// Per-file occurrence counts, in first-seen file order.
    pub fn file_rollup(&self) -> Vec<FileCount> {
        let mut order: Vec<String> = Vec::new();
        let mut counts: HashMap<String, (usize, ScopeClassification)> = HashMap::new();
        for occ in &self.occurrences {
            let entry = counts.entry(occ.file.clone()).or_insert_with(|| {
                order.push(occ.file.clone());
                (0, occ.scope_classification)
            });
            entry.0 += 1;
        }
        order
            .into_iter()
            .map(|file| {
                let (count, scope_classification) = counts
                    .get(&file)
                    .copied()
                    .unwrap_or((0, ScopeClassification::Unknown));
                FileCount {
                    file,
                    count,
                    scope_classification,
                }
            })
            .collect()
    }

    /// Apply [`ReportOptions`] in place. Must be called on the full result set
    /// (before any truncation) so `total`/`by_file` reflect every occurrence.
    pub fn apply_report(&mut self, report: ReportOptions) {
        if report.group_by_file {
            self.by_file = Some(self.file_rollup());
        }
        if let Some(limit) = report.limit {
            // Page metadata is computed against `self.total` (the full result
            // count), but the slice indices must be clamped to the actual
            // backing length to avoid an out-of-bounds panic when
            // `self.total > self.occurrences.len()` (e.g. apply_report called
            // twice or on an already-slimmed result). When the invariant
            // `total == len` holds, behavior is unchanged.
            let len = self.occurrences.len();
            let offset = report.offset.min(len);
            let end = offset.saturating_add(limit).min(len);
            let returned = end.saturating_sub(offset);
            let has_more = end < self.total;
            self.occurrences = self.occurrences[offset..end].to_vec();
            self.page = Some(OccurrencePage {
                offset,
                limit,
                returned,
                has_more,
                next_offset: has_more.then_some(end),
            });
        } else if report.offset > 0 {
            // Page metadata uses `self.total`; the slice index is clamped to
            // the backing length to stay panic-safe (see the limit branch).
            let offset = report.offset.min(self.total);
            let returned = self.total.saturating_sub(offset);
            let slice_offset = report.offset.min(self.occurrences.len());
            self.occurrences = self.occurrences[slice_offset..].to_vec();
            self.page = Some(OccurrencePage {
                offset,
                limit: returned,
                returned,
                has_more: false,
                next_offset: None,
            });
        }
        if report.count_only {
            self.slim = true;
            self.occurrences.clear();
        }
    }
}
