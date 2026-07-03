use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::symbols::{
    Confidence, LanguageId, OccurrenceRole, SymbolGraph, SymbolId, SymbolKind, SymbolNode,
    SymbolOccurrence, SymbolProvenance, SymbolVisibility, TextRange,
};

const INDEXSTORE_DUMP_HELPER: &str = "loctree-indexstore-dump";
const INDEXSTORE_TEST_DUMP_HELPER: &str = "dump-indexstore.sh";

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DumpRecord {
    Symbol(DumpSymbol),
    Occurrence(DumpOccurrence),
}

#[derive(Debug, Deserialize)]
struct DumpSymbol {
    usr: String,
    name: String,
    #[serde(default)]
    qualified_name: Option<String>,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    symbol_kind: Option<String>,
    #[serde(default)]
    file: Option<PathBuf>,
    #[serde(default)]
    range: DumpRange,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DumpOccurrence {
    usr: String,
    file: PathBuf,
    #[serde(default)]
    range: DumpRange,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DumpRange {
    #[serde(default)]
    start_byte: usize,
    #[serde(default)]
    end_byte: usize,
    #[serde(default)]
    start_line: usize,
    #[serde(default)]
    start_col: usize,
    #[serde(default)]
    end_line: usize,
    #[serde(default)]
    end_col: usize,
}

pub(super) fn read_store_with_command(
    root: &Path,
    store: &Path,
    dump_command: &Path,
) -> io::Result<SymbolGraph> {
    // Trust boundary: production input comes from LOCTREE_INDEXSTORE_DUMP.
    // Keep it to a known helper name and canonicalize absolute helper paths
    // before spawning; stores remain argv data, never shell text.
    let dump_command = validated_dump_command(dump_command)?;
    let output = Command::new(&dump_command)
        .arg(store)
        .output()
        .map_err(|err| {
            io::Error::other(format!(
                "failed to run IndexStore dump command {}: {err}",
                dump_command.display()
            ))
        })?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "IndexStore dump command {} failed with status {}: {}",
            dump_command.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|err| {
        io::Error::other(format!(
            "IndexStore dump command {} emitted non-UTF8 output: {err}",
            dump_command.display()
        ))
    })?;
    parse_dump_jsonl(root, &stdout)
}

fn validated_dump_command(dump_command: &Path) -> io::Result<PathBuf> {
    let name = dump_command
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "IndexStore dump command must be valid UTF-8",
            )
        })?;
    let helper = allowed_dump_helper(name)?;

    if dump_command.to_string_lossy().contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "IndexStore dump command contains NUL byte",
        ));
    }
    if dump_command.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "IndexStore dump command may not contain traversal components",
        ));
    }

    if dump_command.is_absolute() {
        let canonical = dump_command.canonicalize()?;
        if !canonical.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "IndexStore dump command is not a file: {}",
                    canonical.display()
                ),
            ));
        }
        return Ok(canonical);
    }

    if dump_command.components().count() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "IndexStore dump command must be a helper name or absolute path",
        ));
    }
    match helper {
        DumpHelper::LoctreeIndexStoreDump => Ok(PathBuf::from("loctree-indexstore-dump")),
        DumpHelper::FixtureDumpIndexStore => Ok(PathBuf::from("dump-indexstore.sh")),
    }
}

#[derive(Clone, Copy)]
enum DumpHelper {
    LoctreeIndexStoreDump,
    FixtureDumpIndexStore,
}

fn allowed_dump_helper(name: &str) -> io::Result<DumpHelper> {
    match name {
        INDEXSTORE_DUMP_HELPER => Ok(DumpHelper::LoctreeIndexStoreDump),
        INDEXSTORE_TEST_DUMP_HELPER => Ok(DumpHelper::FixtureDumpIndexStore),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported IndexStore dump helper `{name}`"),
        )),
    }
}

fn parse_dump_jsonl(root: &Path, text: &str) -> io::Result<SymbolGraph> {
    let mut graph = SymbolGraph::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let record: DumpRecord = serde_json::from_str(trimmed).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid IndexStore JSONL at line {}: {err}", idx + 1),
            )
        })?;
        match record {
            DumpRecord::Symbol(symbol) => graph.symbols.push(symbol.into_node(root)),
            DumpRecord::Occurrence(occurrence) => {
                graph.occurrences.push(occurrence.into_occurrence(root));
            }
        }
    }
    Ok(graph)
}

impl DumpSymbol {
    fn into_node(self, root: &Path) -> SymbolNode {
        let file = self.file.map(|path| normalize_path(root, path));
        let language = self
            .language
            .as_deref()
            .map(parse_language)
            .or_else(|| file.as_deref().and_then(language_from_path))
            .unwrap_or(LanguageId::Swift);
        let kind = self
            .symbol_kind
            .as_deref()
            .map(parse_symbol_kind)
            .unwrap_or(SymbolKind::Other("unknown".to_string()));
        SymbolNode {
            id: SymbolId::new(self.usr.clone()),
            language,
            kind,
            name: self.name,
            qualified_name: self.qualified_name,
            module: self.module,
            usr: Some(self.usr),
            file,
            range: Some(self.range.into_text_range()),
            signature: self.signature,
            visibility: self.visibility.as_deref().map(parse_visibility),
            provenance: SymbolProvenance::IndexStore,
        }
    }
}

impl DumpOccurrence {
    fn into_occurrence(self, root: &Path) -> SymbolOccurrence {
        SymbolOccurrence {
            symbol_id: SymbolId::new(self.usr),
            file: normalize_path(root, self.file),
            range: self.range.into_text_range(),
            role: self
                .role
                .as_deref()
                .map(parse_role)
                .unwrap_or(OccurrenceRole::Reference),
            confidence: Confidence::Precise,
            engine: SymbolProvenance::IndexStore,
        }
    }
}

impl DumpRange {
    fn into_text_range(self) -> TextRange {
        TextRange {
            start_byte: self.start_byte,
            end_byte: self.end_byte,
            start_line: self.start_line,
            start_col: self.start_col,
            end_line: self.end_line,
            end_col: self.end_col,
        }
    }
}

fn normalize_path(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute()
        && let Ok(stripped) = path.strip_prefix(root)
    {
        return stripped.to_path_buf();
    }
    path
}

fn language_from_path(path: &Path) -> Option<LanguageId> {
    match path.extension().and_then(|s| s.to_str()) {
        Some("swift") => Some(LanguageId::Swift),
        Some("m") => Some(LanguageId::ObjC),
        Some("mm") => Some(LanguageId::ObjCpp),
        _ => None,
    }
}

fn parse_language(value: &str) -> LanguageId {
    match value {
        "swift" | "Swift" => LanguageId::Swift,
        "objc" | "objective_c" | "Objective-C" => LanguageId::ObjC,
        "objcpp" | "objective_cpp" | "Objective-C++" => LanguageId::ObjCpp,
        other => language_from_path(Path::new(other)).unwrap_or(LanguageId::Swift),
    }
}

fn parse_symbol_kind(value: &str) -> SymbolKind {
    match value {
        "type" => SymbolKind::Type,
        "class" => SymbolKind::Class,
        "struct" => SymbolKind::Struct,
        "protocol" => SymbolKind::Protocol,
        "enum" => SymbolKind::Enum,
        "func" | "function" => SymbolKind::Func,
        "method" => SymbolKind::Method,
        "property" => SymbolKind::Property,
        "field" => SymbolKind::Field,
        "var" | "variable" => SymbolKind::Var,
        "selector" => SymbolKind::Selector,
        "module" => SymbolKind::Module,
        other => SymbolKind::Other(other.to_string()),
    }
}

fn parse_role(value: &str) -> OccurrenceRole {
    match value {
        "definition" | "def" => OccurrenceRole::Definition,
        "declaration" | "decl" => OccurrenceRole::Declaration,
        "call" => OccurrenceRole::Call,
        "import" => OccurrenceRole::Import,
        _ => OccurrenceRole::Reference,
    }
}

fn parse_visibility(value: &str) -> SymbolVisibility {
    match value {
        "open" => SymbolVisibility::Open,
        "public" => SymbolVisibility::Public,
        "package" => SymbolVisibility::Package,
        "internal" => SymbolVisibility::Internal,
        "fileprivate" => SymbolVisibility::FilePrivate,
        "private" => SymbolVisibility::Private,
        _ => SymbolVisibility::Unknown,
    }
}
