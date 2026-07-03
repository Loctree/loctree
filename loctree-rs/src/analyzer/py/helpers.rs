//! Byte manipulation helper functions for Python parsing.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

/// True when `name` is a valid Python identifier (`[A-Za-z_][A-Za-z0-9_]*`).
///
/// Defensive check used after regex/strip-prefix-based symbol extraction to
/// reject text that slipped in from string literals (e.g. JS `class Foo {`
/// embedded in a Python f-string with the `{{` brace escape, captured as
/// `"Foo {{"`). Empty strings, unicode-letter identifiers (PEP 3131), and
/// anything containing whitespace, braces, parens, or punctuation are all
/// rejected here. This is intentionally narrower than the Python language
/// spec — public API symbols in real-world code are overwhelmingly ASCII.
#[inline]
pub(super) fn is_valid_python_identifier(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Extract the target identifier of a simple module-level assignment.
///
/// Recognizes `NAME = ...` and `NAME: Type = ...` (the shapes module-level
/// constants take, e.g. `FRAMEWORK_LAUNCHER_MARKERS = (...)`). Returns `None`
/// for anything that is not a plain single-identifier binding so the module
/// symbol index stays clean:
/// - comparisons / walrus / augmented assignment (`==`, `:=`, `+=`, ...),
/// - subscript or attribute targets (`cfg["k"] = 1`, `obj.attr = 1`),
/// - tuple/parallel unpacking (`a, b = ...`).
///
/// Caller must enforce module scope (indent == 0); this only validates shape.
pub(super) fn parse_module_const_target(trimmed: &str) -> Option<&str> {
    let bytes = trimmed.as_bytes();
    // Find the first `=` that is a plain assignment operator.
    let mut i = 0;
    let eq_pos = loop {
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b'=' {
            let next = bytes.get(i + 1).copied().unwrap_or(b' ');
            if next == b'=' {
                // `==` comparison: skip both chars and keep scanning.
                i += 2;
                continue;
            }
            let prev = if i > 0 { bytes[i - 1] } else { b' ' };
            // Augmented assignment / comparison / walrus: the char before `=`
            // is an operator. A plain assignment has an identifier or space.
            if matches!(
                prev,
                b'!' | b'<'
                    | b'>'
                    | b'+'
                    | b'-'
                    | b'*'
                    | b'/'
                    | b'%'
                    | b'&'
                    | b'|'
                    | b'^'
                    | b'@'
                    | b':'
                    | b'~'
                    | b'='
            ) {
                return None;
            }
            break i;
        }
        i += 1;
    };

    let lhs = trimmed[..eq_pos].trim();
    // Strip an optional type annotation: `NAME: Type`.
    let name = lhs.split(':').next().unwrap_or(lhs).trim();
    if is_valid_python_identifier(name) {
        Some(name)
    } else {
        None
    }
}

/// Helper to safely compare bytes at position with a keyword.
/// Returns true if the bytes at position match the keyword.
#[inline]
pub(super) fn bytes_match_keyword(bytes: &[u8], pos: usize, keyword: &[u8]) -> bool {
    if pos + keyword.len() > bytes.len() {
        return false;
    }
    &bytes[pos..pos + keyword.len()] == keyword
}

/// Helper to safely extract an ASCII identifier from bytes.
/// Returns the identifier as a string if valid ASCII, empty string otherwise.
#[inline]
pub(super) fn extract_ascii_ident(bytes: &[u8], start: usize, end: usize) -> String {
    if start >= end || end > bytes.len() {
        return String::new();
    }
    // Only extract if all bytes are valid ASCII identifier chars
    let slice = &bytes[start..end];
    if slice.iter().all(|b| b.is_ascii()) {
        String::from_utf8_lossy(slice).into_owned()
    } else {
        String::new()
    }
}

/// Python keywords and builtins to skip when detecting identifiers.
pub(super) const SKIP_BUILTINS: &[&str] = &[
    "None",
    "True",
    "False",
    "str",
    "int",
    "float",
    "bool",
    "bytes",
    "list",
    "dict",
    "set",
    "tuple",
    "frozenset",
    "type",
    "object",
    "Any",
    "Union",
    "Optional",
    "List",
    "Dict",
    "Set",
    "Tuple",
    "Callable",
    "Sequence",
    "Mapping",
    "Iterable",
    "Iterator",
    "Type",
    "self",
    "cls",
];

/// Extended skip list for type hint extraction.
pub(super) const SKIP_TYPE_HINTS: &[&str] = &[
    "None",
    "True",
    "False",
    "str",
    "int",
    "float",
    "bool",
    "bytes",
    "list",
    "dict",
    "set",
    "tuple",
    "frozenset",
    "type",
    "object",
    "Any",
    "Union",
    "Optional",
    "List",
    "Dict",
    "Set",
    "Tuple",
    "Callable",
    "Sequence",
    "Mapping",
    "Iterable",
    "Iterator",
    "Generator",
    "Coroutine",
    "Awaitable",
    "AsyncIterator",
    "AsyncGenerator",
    "Type",
    "ClassVar",
    "Final",
    "Literal",
    "TypeVar",
    "Generic",
    "Protocol",
    "Self",
    "self",
    "cls",
];

/// Python keywords that look like function calls but aren't.
pub(super) const PYTHON_KEYWORDS: &[&str] = &[
    "if", "else", "elif", "while", "for", "try", "except", "finally", "with", "as", "def", "class",
    "return", "yield", "raise", "import", "from", "pass", "break", "continue", "lambda", "and",
    "or", "not", "in", "is", "True", "False", "None", "assert", "del", "exec", "print", "global",
    "nonlocal", "async", "await",
];

/// Known type containers that take types as parameters.
pub(super) const TYPE_FACTORIES: &[&str] = &[
    "defaultdict",
    "Counter",
    "deque",
    "OrderedDict",
    "ChainMap",
    "namedtuple",
    "TypedDict",
    "NewType",
    "cast",
    // FastAPI dependency injection
    "Depends",
    "Security",
    // Pydantic
    "Field",
];

#[cfg(test)]
mod tests {
    use super::is_valid_python_identifier;

    #[test]
    fn accepts_plain_ascii_identifiers() {
        assert!(is_valid_python_identifier("Foo"));
        assert!(is_valid_python_identifier("foo_bar"));
        assert!(is_valid_python_identifier("_private"));
        assert!(is_valid_python_identifier("Foo123"));
        assert!(is_valid_python_identifier("__init__"));
        assert!(is_valid_python_identifier("a"));
    }

    #[test]
    fn rejects_fstring_class_capture() {
        // The exact false-positive shape from the ScreenScribe context-tool
        // bug report: JS `class FrameMarker {` embedded in a Python f-string
        // arrives at the parser as `FrameMarker {{`.
        assert!(!is_valid_python_identifier("FrameMarker {{"));
        assert!(!is_valid_python_identifier("VoiceRecorder {{"));
        assert!(!is_valid_python_identifier("Foo {"));
    }

    #[test]
    fn rejects_invalid_first_chars_and_punctuation() {
        assert!(!is_valid_python_identifier(""));
        assert!(!is_valid_python_identifier("123abc"));
        assert!(!is_valid_python_identifier("Foo Bar"));
        assert!(!is_valid_python_identifier("Foo-Bar"));
        assert!(!is_valid_python_identifier("Foo.Bar"));
        assert!(!is_valid_python_identifier("Foo()"));
        assert!(!is_valid_python_identifier(" Foo"));
    }
}
