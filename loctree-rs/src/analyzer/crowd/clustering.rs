//! Name-based clustering for crowd detection

use regex::Regex;
use std::collections::HashMap;

/// Find files whose names or exports match a pattern
pub fn cluster_by_name(files: &[crate::types::FileAnalysis], pattern: &str) -> Vec<String> {
    let re = Regex::new(&format!("(?i){}", pattern))
        .unwrap_or_else(|_| Regex::new(&regex::escape(pattern)).unwrap());

    let mut matches = Vec::new();

    for file in files {
        // Check file name
        if re.is_match(&file.path) {
            matches.push(file.path.clone());
            continue;
        }

        // Check export names
        for export in &file.exports {
            if re.is_match(&export.name) {
                matches.push(file.path.clone());
                break;
            }
        }
    }

    matches
}

/// Auto-detect potential crowds by finding common name patterns
pub fn detect_name_patterns(files: &[crate::types::FileAnalysis]) -> Vec<String> {
    let mut word_counts: HashMap<String, usize> = HashMap::new();

    // Extract significant words from file names and exports
    let word_re = Regex::new(r"[A-Z][a-z]+|[a-z]+").unwrap();

    for file in files {
        // From file name
        let filename = file.path.rsplit('/').next().unwrap_or(&file.path);
        for cap in word_re.find_iter(filename) {
            let word = cap.as_str().to_lowercase();
            if word.len() > 3 {
                // Skip short words
                *word_counts.entry(word).or_insert(0) += 1;
            }
        }

        // From exports
        for export in &file.exports {
            for cap in word_re.find_iter(&export.name) {
                let word = cap.as_str().to_lowercase();
                if word.len() > 3 {
                    *word_counts.entry(word).or_insert(0) += 1;
                }
            }
        }
    }

    // Return words that appear in 3+ files (potential crowds)
    let mut patterns: Vec<_> = word_counts
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .collect();
    patterns.sort_by_key(|b| std::cmp::Reverse(b.1));

    patterns.into_iter().map(|(word, _)| word).collect()
}
