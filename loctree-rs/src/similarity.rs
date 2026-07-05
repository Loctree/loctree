pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.chars().count();
    let b_len = b.chars().count();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut d = vec![vec![0; b_len + 1]; a_len + 1];

    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, val) in d[0].iter_mut().enumerate() {
        *val = j;
    }

    for (i, ca) in a.chars().enumerate() {
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            d[i + 1][j + 1] = std::cmp::min(
                std::cmp::min(d[i][j + 1] + 1, d[i + 1][j] + 1),
                d[i][j] + cost,
            );
        }
    }

    d[a_len][b_len]
}

pub fn similarity(a: &str, b: &str) -> f64 {
    let dist = levenshtein(a, b);
    let max_len = std::cmp::max(a.chars().count(), b.chars().count());
    if max_len == 0 {
        1.0
    } else {
        1.0 - (dist as f64 / max_len as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_identical_strings() {
        assert_eq!(levenshtein("hello", "hello"), 0);
        assert_eq!(levenshtein("", ""), 0);
    }

    #[test]
    fn levenshtein_empty_first() {
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn levenshtein_empty_second() {
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn levenshtein_single_edit() {
        assert_eq!(levenshtein("cat", "bat"), 1); // substitution
        assert_eq!(levenshtein("cat", "cats"), 1); // insertion
        assert_eq!(levenshtein("cats", "cat"), 1); // deletion
    }

    #[test]
    fn levenshtein_multiple_edits() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("saturday", "sunday"), 3);
    }

    #[test]
    fn similarity_identical() {
        assert!((similarity("hello", "hello") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn similarity_empty_both() {
        assert!((similarity("", "") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn similarity_completely_different() {
        assert!((similarity("abc", "xyz") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn similarity_partial_match() {
        let sim = similarity("hello", "hallo");
        assert!(sim > 0.7 && sim < 0.9); // 4/5 = 0.8
    }
}
