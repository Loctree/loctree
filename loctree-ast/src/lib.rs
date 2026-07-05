//! Narrow tree-sitter substrate for Loctree live-AST and structural query paths.
//!
//! This crate intentionally runs alongside the existing analyzer stack. It does
//! not replace OXC or the cold-scan extractors; it gives LSP/runtime code a
//! typed parser boundary that can grow without disturbing snapshot generation.

use std::path::Path;

pub mod extractors;

pub use extractors::{
    CallEntry, ExportSymbol as ExtractedExport, ImportBinding, ImportEntry as ExtractedImport,
    JsExtractor, LangExtractor, PyExtractor, TsExtractor,
};
pub use tree_sitter::{
    InputEdit, Language, Parser, Point, Query, QueryCursor, StreamingIterator, Tree,
};

/// Parsed source plus the tree-sitter tree and Loctree language id.
pub struct LoctreeTree {
    pub tree: Tree,
    pub source: Vec<u8>,
    pub lang: &'static str,
}

impl LoctreeTree {
    pub fn root_kind(&self) -> &str {
        self.tree.root_node().kind()
    }

    pub fn has_error(&self) -> bool {
        self.tree.root_node().has_error()
    }
}

/// Object-safe parser metadata used by the registry.
pub trait LangParser: Send + Sync {
    fn language(&self) -> Language;
    fn lang_id(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
}

#[derive(Debug, thiserror::Error)]
pub enum AstError {
    #[error("unsupported AST language: {0}")]
    UnsupportedLanguage(String),
    #[error("tree-sitter rejected {lang} grammar: {source}")]
    Language {
        lang: &'static str,
        source: tree_sitter::LanguageError,
    },
    #[error("tree-sitter could not parse {lang} source")]
    ParseFailed { lang: &'static str },
}

pub struct Parsers {
    parsers: Vec<Box<dyn LangParser>>,
}

impl Default for Parsers {
    fn default() -> Self {
        Self::new_default()
    }
}

impl Parsers {
    pub fn new_default() -> Self {
        Self {
            parsers: vec![
                Box::new(JavaScriptParser),
                Box::new(PythonParser),
                Box::new(TypeScriptParser),
                Box::new(TsxParser),
            ],
        }
    }

    pub fn language_ids(&self) -> Vec<&'static str> {
        self.parsers.iter().map(|parser| parser.lang_id()).collect()
    }

    pub fn lookup(&self, lang_id: &str) -> Option<&dyn LangParser> {
        let normalized = normalize_lang_id(lang_id);
        self.parsers
            .iter()
            .find(|parser| parser.lang_id() == normalized)
            .map(|parser| parser.as_ref())
    }

    pub fn for_path(&self, path: &Path) -> Option<&dyn LangParser> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        self.parsers
            .iter()
            .find(|parser| parser.extensions().contains(&ext.as_str()))
            .map(|parser| parser.as_ref())
    }

    pub fn parse(&self, lang: &dyn LangParser, source: &[u8]) -> Result<LoctreeTree, AstError> {
        let mut parser = Parser::new();
        let language = lang.language();
        parser
            .set_language(&language)
            .map_err(|source| AstError::Language {
                lang: lang.lang_id(),
                source,
            })?;
        let tree = parser.parse(source, None).ok_or(AstError::ParseFailed {
            lang: lang.lang_id(),
        })?;
        Ok(LoctreeTree {
            tree,
            source: source.to_vec(),
            lang: lang.lang_id(),
        })
    }

    pub fn parse_path(&self, path: &Path, source: &[u8]) -> Result<LoctreeTree, AstError> {
        let lang = self.for_path(path).ok_or_else(|| {
            AstError::UnsupportedLanguage(path.extension_id().unwrap_or("unknown").to_string())
        })?;
        self.parse(lang, source)
    }

    pub fn parse_language(&self, lang_id: &str, source: &[u8]) -> Result<LoctreeTree, AstError> {
        let lang = self
            .lookup(lang_id)
            .ok_or_else(|| AstError::UnsupportedLanguage(lang_id.to_string()))?;
        self.parse(lang, source)
    }

    pub fn parse_incremental(
        &self,
        prev: &LoctreeTree,
        new_source: &[u8],
        edits: &[InputEdit],
    ) -> Result<LoctreeTree, AstError> {
        let lang = self
            .lookup(prev.lang)
            .ok_or_else(|| AstError::UnsupportedLanguage(prev.lang.to_string()))?;
        let mut old_tree = prev.tree.clone();
        for edit in edits {
            old_tree.edit(edit);
        }

        let mut parser = Parser::new();
        let language = lang.language();
        parser
            .set_language(&language)
            .map_err(|source| AstError::Language {
                lang: lang.lang_id(),
                source,
            })?;
        let tree = parser
            .parse(new_source, Some(&old_tree))
            .ok_or(AstError::ParseFailed {
                lang: lang.lang_id(),
            })?;

        Ok(LoctreeTree {
            tree,
            source: new_source.to_vec(),
            lang: lang.lang_id(),
        })
    }
}

struct JavaScriptParser;
struct PythonParser;
struct TypeScriptParser;
struct TsxParser;

impl LangParser for JavaScriptParser {
    fn language(&self) -> Language {
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn lang_id(&self) -> &'static str {
        "javascript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["js", "cjs", "mjs", "jsx"]
    }
}

impl LangParser for PythonParser {
    fn language(&self) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn lang_id(&self) -> &'static str {
        "python"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py", "pyi"]
    }
}

impl LangParser for TypeScriptParser {
    fn language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn lang_id(&self) -> &'static str {
        "typescript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "cts", "mts"]
    }
}

impl LangParser for TsxParser {
    fn language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    }

    fn lang_id(&self) -> &'static str {
        "tsx"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["tsx"]
    }
}

fn normalize_lang_id(lang_id: &str) -> &str {
    match lang_id {
        "js" | "jsx" | "node" => "javascript",
        "py" => "python",
        "ts" => "typescript",
        other => other,
    }
}

trait PathExtensionId {
    fn extension_id(&self) -> Option<&str>;
}

impl PathExtensionId for Path {
    fn extension_id(&self) -> Option<&str> {
        self.extension().and_then(|ext| ext.to_str())
    }
}
