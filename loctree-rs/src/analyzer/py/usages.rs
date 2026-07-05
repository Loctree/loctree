//! Python symbol usage extraction.
//!
//! Handles extraction of symbol usages from:
//! - Type hints (annotations, generics, factory patterns)
//! - Container literals (tuples, lists, dicts)
//! - Function calls
//! - Bare class references (return statements, isinstance, issubclass, raise)
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use super::helpers::{
    PYTHON_KEYWORDS, SKIP_BUILTINS, SKIP_TYPE_HINTS, TYPE_FACTORIES, bytes_match_keyword,
    extract_ascii_ident,
};

/// Extract identifiers used in type hints from Python code.
/// This catches patterns like `x: MyClass`, `def foo(x: MyClass)`, `List[MyClass]`, `dict[str, MyClass]`
/// Also catches factory patterns like `defaultdict(MyClass)`, `set(MyClass)` etc.
pub(super) fn extract_type_hint_usages(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();

    let mut i = 0;
    while i < len {
        // Look for `:` or `->` followed by type annotation
        if bytes[i] == b':' || (i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'>') {
            if bytes[i] == b'-' {
                i += 2; // skip `->`
            } else {
                i += 1; // skip `:`
            }

            // Skip whitespace
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Now extract identifiers from the type annotation
            // Handle nested brackets for generics like Dict[str, List[MyClass]]
            let mut bracket_depth = 0;
            let start_pos = i;

            while i < len {
                match bytes[i] {
                    b'[' => {
                        bracket_depth += 1;
                        i += 1;
                    }
                    b']' => {
                        if bracket_depth > 0 {
                            bracket_depth -= 1;
                        }
                        i += 1;
                    }
                    b',' | b')' | b'\n' | b'#' | b'=' if bracket_depth == 0 => {
                        break;
                    }
                    _ if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' => {
                        let ident_start = i;
                        while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                            i += 1;
                        }
                        let ident = extract_ascii_ident(bytes, ident_start, i);
                        if !ident.is_empty()
                            && !SKIP_TYPE_HINTS.contains(&ident.as_str())
                            && !local_uses.contains(&ident)
                        {
                            local_uses.push(ident);
                        }
                    }
                    _ => {
                        i += 1;
                    }
                }

                // Stop if we've gone too far (reasonable limit for type annotations)
                if i - start_pos > 500 {
                    break;
                }
            }
        }
        // Look for factory calls like defaultdict(MyClass)
        else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let ident_start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = extract_ascii_ident(bytes, ident_start, i);

            if TYPE_FACTORIES.contains(&ident.as_str()) {
                // Skip whitespace
                while i < len && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                // Check for opening paren
                if i < len && bytes[i] == b'(' {
                    i += 1;
                    // Skip whitespace
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    // Extract the type argument
                    if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                        let type_start = i;
                        while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                            i += 1;
                        }
                        let type_ident = extract_ascii_ident(bytes, type_start, i);
                        if !type_ident.is_empty()
                            && !SKIP_TYPE_HINTS.contains(&type_ident.as_str())
                            && !local_uses.contains(&type_ident)
                        {
                            local_uses.push(type_ident);
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Extract class references from tuple/list/dict literals.
/// This catches patterns like:
/// - `(ClassName, 'value')` - tuple literals
/// - `[ClassName, other]` - list literals
/// - `{'key': ClassName}` - dict values
/// - `self.classes = (Foo, Bar)` - class attribute assignments
pub(super) fn extract_class_from_containers(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let ch = bytes[i];

        // Look for container opening: ( [ {
        if ch == b'(' || ch == b'[' || ch == b'{' {
            i += 1;
            let closing = match ch {
                b'(' => b')',
                b'[' => b']',
                b'{' => b'}',
                _ => unreachable!(),
            };

            // Parse identifiers within the container
            let mut depth = 1;
            while i < len && depth > 0 {
                match bytes[i] {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' if bytes[i] == closing => {
                        depth -= 1;
                    }
                    b'\'' | b'"' => {
                        // Skip string literals
                        let quote = bytes[i];
                        i += 1;
                        while i < len && bytes[i] != quote {
                            if bytes[i] == b'\\' {
                                i += 1; // Skip escaped character
                            }
                            i += 1;
                        }
                    }
                    _ if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' => {
                        let start = i;
                        while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                            i += 1;
                        }
                        let ident = extract_ascii_ident(bytes, start, i);

                        // Skip if followed by '=' (dict key) or '(' (function call)
                        let mut j = i;
                        while j < len && bytes[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        let is_dict_key = j < len && bytes[j] == b'=';
                        let is_function_call = j < len && bytes[j] == b'(';

                        if !ident.is_empty()
                            && !is_dict_key
                            && !is_function_call
                            && !SKIP_BUILTINS.contains(&ident.as_str())
                            && !local_uses.contains(&ident)
                        {
                            local_uses.push(ident);
                        }
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
}

/// Extract function calls from Python code to detect local usage.
/// This catches patterns like `func_name(...)` which indicate the function is used.
pub(super) fn extract_python_function_calls(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for identifier followed by `(`
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = extract_ascii_ident(bytes, start, i);

            // Skip whitespace
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }

            // Check if followed by `(`
            if i < len
                && bytes[i] == b'('
                && !ident.is_empty()
                && !PYTHON_KEYWORDS.contains(&ident.as_str())
                && !local_uses.contains(&ident)
            {
                local_uses.push(ident);
            }
        } else {
            i += 1;
        }
    }
}

/// Extract bare class name references from Python code.
/// This catches patterns like:
/// - `return ClassName` - bare class name in return statement
/// - `issubclass(x, ClassName)` - class as function argument
/// - `isinstance(obj, MyClass)` - class as function argument
/// - `raise CustomError` - exception class names
pub(super) fn extract_bare_class_references(content: &str, local_uses: &mut Vec<String>) {
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for "return" keyword (use byte comparison, not string slicing)
        if bytes_match_keyword(bytes, i, b"return") {
            // Check it's a word boundary
            if i == 0 || !bytes[i - 1].is_ascii_alphanumeric() {
                i += 6;
                // Skip whitespace
                while i < len && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                // Extract identifier after return
                if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                    let start = i;
                    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                    let ident = extract_ascii_ident(bytes, start, i);
                    if !ident.is_empty()
                        && !SKIP_BUILTINS.contains(&ident.as_str())
                        && !local_uses.contains(&ident)
                    {
                        local_uses.push(ident);
                    }
                }
                continue;
            }
        }

        // Look for "raise" keyword (exception class names)
        if bytes_match_keyword(bytes, i, b"raise")
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            i += 5;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                let start = i;
                while i < len
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
                {
                    i += 1;
                }
                let ident = extract_ascii_ident(bytes, start, i);
                // Extract last component if dotted
                let simple = ident.rsplit('.').next().unwrap_or(&ident);
                if !ident.is_empty()
                    && !SKIP_BUILTINS.contains(&simple)
                    && !local_uses.contains(&simple.to_string())
                {
                    local_uses.push(simple.to_string());
                }
            }
            continue;
        }

        // Look for isinstance/issubclass calls (use byte comparison)
        let is_isinstance = bytes_match_keyword(bytes, i, b"isinstance");
        let is_issubclass = bytes_match_keyword(bytes, i, b"issubclass");
        if (is_isinstance || is_issubclass) && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            i += 10; // Both "isinstance" and "issubclass" are 10 chars
            // Skip whitespace and opening paren
            while i < len && (bytes[i].is_ascii_whitespace() || bytes[i] == b'(') {
                i += 1;
            }

            // Skip first argument (object/class to check)
            let mut paren_depth = 0;
            while i < len {
                match bytes[i] {
                    b'(' => paren_depth += 1,
                    b')' => {
                        if paren_depth == 0 {
                            break;
                        }
                        paren_depth -= 1;
                    }
                    b',' if paren_depth == 0 => {
                        i += 1;
                        break;
                    }
                    _ => {}
                }
                i += 1;
            }

            // Now extract the class name (second argument)
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                let start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let ident = extract_ascii_ident(bytes, start, i);
                if !ident.is_empty()
                    && !SKIP_BUILTINS.contains(&ident.as_str())
                    && !local_uses.contains(&ident)
                {
                    local_uses.push(ident);
                }
            }
            continue;
        }

        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_hint_skips_builtins() {
        let mut uses = Vec::new();
        let content = "def foo(x: str, y: int) -> bool: pass";
        extract_type_hint_usages(content, &mut uses);

        // Should NOT contain builtins
        assert!(!uses.contains(&"str".to_string()));
        assert!(!uses.contains(&"int".to_string()));
        assert!(!uses.contains(&"bool".to_string()));
    }

    #[test]
    fn detects_nested_generic_type_hints() {
        let mut uses = Vec::new();
        let content = "cache: Dict[str, List[MyClass]] = {}";
        extract_type_hint_usages(content, &mut uses);

        assert!(
            uses.contains(&"MyClass".to_string()),
            "MyClass not found in: {:?}",
            uses
        );
    }

    #[test]
    fn detects_class_in_tuple_literal() {
        let mut uses = Vec::new();
        let content = r#"
class StringTypePrinter: pass
class SliceTypePrinter: pass

how = ((StringTypePrinter, 'len'),
       (SliceTypePrinter, 'len'))
"#;
        extract_class_from_containers(content, &mut uses);

        assert!(
            uses.contains(&"StringTypePrinter".to_string()),
            "StringTypePrinter not found in: {:?}",
            uses
        );
        assert!(
            uses.contains(&"SliceTypePrinter".to_string()),
            "SliceTypePrinter not found in: {:?}",
            uses
        );
    }

    #[test]
    fn detects_class_in_list_literal() {
        let mut uses = Vec::new();
        let content = r#"
class Foo: pass
class Bar: pass

items = [Foo, Bar, 'string']
"#;
        extract_class_from_containers(content, &mut uses);

        assert!(uses.contains(&"Foo".to_string()));
        assert!(uses.contains(&"Bar".to_string()));
        // String literals should be skipped
        assert!(!uses.iter().any(|s| s.contains("string")));
    }

    #[test]
    fn detects_class_in_dict_literal() {
        let mut uses = Vec::new();
        let content = r#"
class Handler: pass
class Parser: pass

mapping = {'handler': Handler, 'parser': Parser}
"#;
        extract_class_from_containers(content, &mut uses);

        assert!(uses.contains(&"Handler".to_string()));
        assert!(uses.contains(&"Parser".to_string()));
    }

    #[test]
    fn skips_builtins_in_containers() {
        let mut uses = Vec::new();
        let content = r#"
types = (str, int, bool, None, True, False)
"#;
        extract_class_from_containers(content, &mut uses);

        // Should not contain any builtins
        assert!(!uses.contains(&"str".to_string()));
        assert!(!uses.contains(&"int".to_string()));
        assert!(!uses.contains(&"bool".to_string()));
        assert!(!uses.contains(&"None".to_string()));
        assert!(!uses.contains(&"True".to_string()));
        assert!(!uses.contains(&"False".to_string()));
    }

    #[test]
    fn detects_function_calls() {
        let mut uses = Vec::new();
        let content = r#"
def helper():
    pass

result = helper()
"#;
        extract_python_function_calls(content, &mut uses);

        assert!(uses.contains(&"helper".to_string()));
    }

    #[test]
    fn skips_keywords_in_function_calls() {
        let mut uses = Vec::new();
        let content = r#"
if condition:
    for item in items:
        pass
"#;
        extract_python_function_calls(content, &mut uses);

        assert!(!uses.contains(&"if".to_string()));
        assert!(!uses.contains(&"for".to_string()));
    }
}
