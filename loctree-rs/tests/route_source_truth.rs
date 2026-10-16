//! Route declarations must be code at original source addresses.

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use tempfile::TempDir;

fn routes_for(file: &str, source: &str) -> Value {
    let root = TempDir::new().expect("fixture root");
    let cache = TempDir::new().expect("private cache");
    std::fs::write(root.path().join(file), source).expect("fixture source");
    let output = cargo_bin_cmd!("loct")
        .current_dir(root.path())
        .env("LOCT_CACHE_DIR", cache.path())
        .env("LOCT_ALLOW_NON_GIT_ROOT", "1")
        .env("LOCT_NO_GITIGNORE", "1")
        .args(["routes", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).expect("route JSON")
}

fn assert_routes(file: &str, source: &str, expected: &[(&str, &str, &str, &str)]) {
    let value = routes_for(file, source);
    let rows = value["routes"].as_array().expect("route rows");
    assert_eq!(rows.len(), expected.len(), "{value:#}");
    for (path, method, name, declaration) in expected {
        let row = rows
            .iter()
            .find(|row| row["path"] == *path)
            .expect("expected route");
        assert_eq!(row["method"], *method);
        assert_eq!(row["handler"], *name);
        assert_eq!(row["file"], file);
        let offset = source.find(declaration).expect("source declaration");
        let line = source[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        assert_eq!(row["line"], line, "source-address mismatch: {row:#}");
    }
}

#[test]
fn rust_routes_are_code_at_original_addresses() {
    let source = r###"//! .route("/doc", get(doc))
/* outer .route("/block", get(block)) /* nested */ */
const EXAMPLE: &str = r##"
.route("/raw", get(raw))
"##;
const ESCAPED: &str = ".route(\"/escaped\", get(fake))";
#[cfg(test)]
mod excluded {
    // 雪 and braces inside a raw string cannot close this module.
    const EXAMPLE: &str = r#" } .route("/test-string", get(fake)) "#;
    fn test_router() {
        Router::new().route("/test", get(test_handler));
    }
}

// π: original line addresses survive the excluded region above.
fn app() {
    let app = Router::new()
        .route("/real", get(real_handler))
        .route(
            "/multiline",
            post(create_handler),
        );
}
"###;
    assert_routes(
        "regexes.rs",
        source,
        &[
            ("/real", "GET", "real_handler", ".route(\"/real\""),
            ("/multiline", "POST", "create_handler", "        .route(\n"),
        ],
    );
}

#[test]
fn python_routes_use_callee_arguments_and_original_addresses() {
    let source = r###"from fastapi import FastAPI, APIRouter
from flask import Blueprint
app = FastAPI()
router = APIRouter()
blueprint = Blueprint("site", __name__)
text = r'''
@app.get("/string")
def fake(): pass
'''
# @app.get("/comment")
@probe("H3b", "the .route( description is not a Flask registration")
def probe_handler(): pass
@probe("@app.get('/description')")
def other_probe(): pass

# 雪 π — a multiline decorator with a # inside the literal path.
@router.post(
    "/雪#fragment",
    description=".route('/not-the-path')",
)
async def create_handler(): pass

@app.get("/real")
def real_handler():
    """@app.get('/docstring')"""
    pass

@blueprint.route(
    rule="/flask",
    methods=["GET", "POST"],
)
def flask_handler(): pass
"###;
    assert_routes(
        "server.py",
        source,
        &[
            ("/雪#fragment", "POST", "create_handler", "@router.post("),
            ("/real", "GET", "real_handler", "@app.get(\"/real\")"),
            ("/flask", "GET,POST", "flask_handler", "@blueprint.route("),
        ],
    );
}

#[test]
fn typescript_express_routes_remain_an_unsupported_control() {
    let source = "import { Router } from 'express';\nexport const router = Router();\nrouter.get('/real', handler);\n";
    assert_routes("router.ts", source, &[]);
}

#[test]
fn python_route_arguments_are_literals_not_description_or_method_substrings() {
    let source = r###"@app.getter("/suffix")
def suffix(): pass
@probe("@app.post('/text')", route=".route('/text')")
def probe_handler(): pass
@app.get(path_variable, description="/not-a-path")
def dynamic(): pass
@app.get(f"/dynamic/{value}")
def interpolated(): pass
@bp.route("/dynamic-method", methods=METHODS)
def dynamic_methods(): pass
@bp.route("/not-literal", methods=[get_method()])
def nonliteral_method(): pass
@app.get("/class")
class NotAHandler: pass
@bp.route('/literal', methods=('get', 'post'))
def literal(): pass
@api_router.put(path=r'/raw')
def raw(): pass
"###;
    assert_routes(
        "probe.py",
        source,
        &[
            ("/literal", "GET,POST", "literal", "@bp.route('/literal'"),
            ("/raw", "PUT", "raw", "@api_router.put("),
        ],
    );
}

#[test]
fn rust_narrow_shapes_keep_chains_qualified_handlers_and_raw_literal_paths() {
    let source = r###"fn router() {
    Router::new()
        .route(r#"/雪"#, get(crate::handlers::list).post(create))
        .route("/quoted\"path", get(quoted))
        .route(path_variable, get(dynamic))
        .route("/closure", get(|| "not a named handler"))
        .route("/factory", get(factory()));
}
"###;
    assert_routes(
        "tests.rs",
        source,
        &[
            ("/雪", "GET", "crate::handlers::list", ".route(r#"),
            ("/quoted\"path", "GET", "quoted", ".route(\"/quoted"),
        ],
    );
}

#[test]
fn rust_cfg_test_regions_do_not_change_addresses_or_exclude_non_test_branches() {
    let excluded = "    // 雪 } string examples cannot end this module\n".repeat(512);
    let source = format!(
        r#"#[cfg(/* explanation */ test)]
mod excluded {{
{excluded}
    fn fixture() {{ Router::new().route("/excluded", get(fake)); }}
}}
#[cfg(all(test, feature = "server"))]
mod also_excluded {{
    fn fixture() {{ Router::new().route("/also-excluded", get(fake)); }}
}}
#[cfg(all(not(test), feature = "server"))]
fn production() {{
    Router::new().route("/production", get(real));
}}
#[cfg(any(test, feature = "server"))]
fn conditional() {{
    Router::new().route("/conditional", get(conditional));
}}
"#
    );
    assert_routes(
        "router.rs",
        &source,
        &[
            ("/production", "GET", "real", ".route(\"/production\""),
            (
                "/conditional",
                "GET",
                "conditional",
                ".route(\"/conditional\"",
            ),
        ],
    );
}
