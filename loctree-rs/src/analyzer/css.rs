use crate::types::{FileAnalysis, ImportEntry, ImportKind};

use super::regexes::regex_css_import;

pub(crate) fn analyze_css_file(content: &str, relative: String) -> FileAnalysis {
    let mut analysis = FileAnalysis::new(relative);
    for caps in regex_css_import().captures_iter(content) {
        let source = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        analysis
            .imports
            .push(ImportEntry::new(source, ImportKind::Static));
    }

    analysis
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_css_imports() {
        let content = r#"
        @import "base.css";
        @import url("theme.css");
        "#;
        let analysis = analyze_css_file(content, "styles/main.css".to_string());
        assert_eq!(analysis.imports.len(), 2);
        assert_eq!(analysis.imports[0].source, "base.css");
        assert_eq!(analysis.imports[1].source, "theme.css");
    }
}
