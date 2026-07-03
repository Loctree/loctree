//! Plan 19 Stage 1 — `LangExtractor` smoke tests for TS/JS/Python.

use loctree_ast::{JsExtractor, LangExtractor, Parsers, PyExtractor, TsExtractor};
use std::path::Path;

#[test]
fn ts_extractor_pulls_function_exports_imports_and_calls() {
    let parsers = Parsers::new_default();
    let source = br#"
import { greet } from './utils/greeting';
import * as fmt from './utils/date';
import Default from './default';
import { other as renamed } from './rename';

export function main(): void {
    console.log(greet('World'));
    fmt.formatDate(new Date());
    renamed();
}

export class Service {}
export const VERSION = '0.10.0';
"#;
    let tree = parsers
        .parse_path(Path::new("demo.ts"), source)
        .expect("ts parses");
    let extractor = TsExtractor;

    let exports = extractor.extract_exports(&tree);
    let names: Vec<&str> = exports.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"main"), "main export missing: {names:?}");
    assert!(names.contains(&"Service"), "Service export missing");
    assert!(names.contains(&"VERSION"), "VERSION export missing");

    let main_export = exports.iter().find(|e| e.name == "main").unwrap();
    assert_eq!(main_export.kind, "function");
    assert_eq!(main_export.export_type, "named");
    assert!(main_export.line.is_some());
    assert!(main_export.byte_range.1 > main_export.byte_range.0);

    let imports = extractor.extract_imports(&tree);
    let sources: Vec<&str> = imports.iter().map(|i| i.source.as_str()).collect();
    assert_eq!(sources.len(), 4, "expected 4 imports, got {sources:?}");
    assert!(sources.contains(&"./utils/greeting"));
    assert!(sources.contains(&"./utils/date"));

    // Default + namespace + named-with-alias bindings.
    let default_imp = imports.iter().find(|i| i.source == "./default").unwrap();
    assert!(
        default_imp
            .symbols
            .iter()
            .any(|b| b.is_default && b.local_name == "Default")
    );
    let ns_imp = imports.iter().find(|i| i.source == "./utils/date").unwrap();
    assert!(
        ns_imp
            .symbols
            .iter()
            .any(|b| b.is_namespace && b.local_name == "fmt")
    );
    let alias_imp = imports.iter().find(|i| i.source == "./rename").unwrap();
    let renamed = alias_imp
        .symbols
        .iter()
        .find(|b| b.local_name == "renamed")
        .expect("renamed binding present");
    assert_eq!(renamed.imported.as_deref(), Some("other"));

    let calls = extractor.extract_calls(&tree);
    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"greet"), "greet call missing: {names:?}");
    assert!(names.contains(&"log"), "console.log call missing");
    assert!(names.contains(&"formatDate"), "fmt.formatDate call missing");
    assert!(names.contains(&"renamed"), "renamed() call missing");
}

#[test]
fn js_extractor_handles_class_and_lexical_exports() {
    let parsers = Parsers::new_default();
    let source = br#"
import { foo } from './foo';
export class Greeter {
    hi() { return 'hi'; }
}
export const VERSION = '0.10.0';
export function bar() { foo(); }
"#;
    let tree = parsers
        .parse_path(Path::new("demo.js"), source)
        .expect("js parses");
    let extractor = JsExtractor;

    let exports = extractor.extract_exports(&tree);
    let names: Vec<&str> = exports.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Greeter"));
    assert!(names.contains(&"VERSION"));
    assert!(names.contains(&"bar"));

    let imports = extractor.extract_imports(&tree);
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].source, "./foo");
    assert!(
        imports[0]
            .symbols
            .iter()
            .any(|b| b.local_name == "foo" && !b.is_default)
    );

    let calls = extractor.extract_calls(&tree);
    assert!(calls.iter().any(|c| c.name == "foo"));
}

#[test]
fn python_extractor_pulls_top_level_exports_imports_all_and_calls() {
    let parsers = Parsers::new_default();
    let source = br#"
import os
import package.module as pm
from . import sibling
from .helpers import util as helper
from pkg.sub import Thing, Other as Alias

__all__ = ["DecoratedService", "async_task", "EXPLICIT_ONLY"]
VALUE = 7

@decorator()
class DecoratedService:
    class Inner:
        pass

    def method(self):
        helper()

async def async_task():
    return pm.make()

def plain():
    sibling.run()

class Standalone:
    pass
"#;
    let tree = parsers
        .parse_path(Path::new("demo.py"), source)
        .expect("python parses");
    let extractor = PyExtractor;

    let exports = extractor.extract_exports(&tree);
    assert_eq!(exports.len(), 8, "exports: {exports:?}");
    let names: Vec<&str> = exports.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"DecoratedService"));
    assert!(names.contains(&"async_task"));
    assert!(names.contains(&"plain"));
    assert!(names.contains(&"Standalone"));
    assert!(names.contains(&"VALUE"));
    assert!(names.contains(&"EXPLICIT_ONLY"));
    assert!(!names.contains(&"Inner"), "nested class leaked as export");
    assert_eq!(
        exports
            .iter()
            .filter(|e| e.export_type == "__all__")
            .count(),
        3
    );
    let async_export = exports
        .iter()
        .find(|e| e.name == "async_task" && e.kind == "function")
        .unwrap();
    assert_eq!(async_export.kind, "function");
    assert!(async_export.byte_range.1 > async_export.byte_range.0);

    let imports = extractor.extract_imports(&tree);
    assert_eq!(imports.len(), 5, "imports: {imports:?}");
    let sources: Vec<&str> = imports.iter().map(|i| i.source.as_str()).collect();
    assert!(sources.contains(&"os"));
    assert!(sources.contains(&"package.module"));
    assert!(sources.contains(&"."));
    assert!(sources.contains(&".helpers"));
    assert!(sources.contains(&"pkg.sub"));
    let dot_import = imports.iter().find(|i| i.source == ".").unwrap();
    assert!(dot_import.symbols.iter().any(|b| b.local_name == "sibling"));
    let helper_import = imports.iter().find(|i| i.source == ".helpers").unwrap();
    let helper = helper_import
        .symbols
        .iter()
        .find(|b| b.local_name == "helper")
        .expect("helper alias binding");
    assert_eq!(helper.imported.as_deref(), Some("util"));
    let pkg_import = imports.iter().find(|i| i.source == "pkg.sub").unwrap();
    assert_eq!(pkg_import.symbols.len(), 2);

    let calls = extractor.extract_calls(&tree);
    assert_eq!(calls.len(), 4, "calls: {calls:?}");
    let call_names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert!(call_names.contains(&"decorator"));
    assert!(call_names.contains(&"helper"));
    assert!(call_names.contains(&"make"));
    assert!(call_names.contains(&"run"));
}

#[test]
fn python_extractor_handles_fastapi_router_shape() {
    let parsers = Parsers::new_default();
    let source = br#"
from fastapi import APIRouter, Depends
from sqlalchemy.orm import Session

router = APIRouter()

class PetOut:
    pass

@router.get("/pets/{pet_id}", response_model=PetOut)
async def read_pet(pet_id: int, db: Session = Depends(get_db)) -> PetOut:
    return load_pet(db, pet_id)

def helper_url():
    return router.url_path_for("read_pet")
"#;
    let tree = parsers
        .parse_path(Path::new("routers/pets.py"), source)
        .expect("fastapi-shaped python parses");
    let extractor = PyExtractor;

    let exports = extractor.extract_exports(&tree);
    let export_names: Vec<&str> = exports.iter().map(|e| e.name.as_str()).collect();
    assert!(export_names.contains(&"router"));
    assert!(export_names.contains(&"PetOut"));
    assert!(export_names.contains(&"read_pet"));
    assert!(export_names.contains(&"helper_url"));

    let imports = extractor.extract_imports(&tree);
    let import_sources: Vec<&str> = imports.iter().map(|i| i.source.as_str()).collect();
    assert_eq!(import_sources, vec!["fastapi", "sqlalchemy.orm"]);

    let calls = extractor.extract_calls(&tree);
    let call_names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert!(call_names.contains(&"APIRouter"));
    assert!(call_names.contains(&"Depends"));
    assert!(call_names.contains(&"get"));
    assert!(call_names.contains(&"load_pet"));
    assert!(call_names.contains(&"url_path_for"));
}
