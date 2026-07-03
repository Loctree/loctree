//! jaq-based query execution for snapshot data.
//!
//! Provides jq-compatible filtering using the jaq library.
//! Allows querying snapshot JSON with filters like `.files[0]`, `.metadata.version`, etc.

use serde_json::Value;
use std::rc::Rc;

/// Executor for jaq (jq-compatible) filters on JSON data.
pub struct JaqExecutor {}

impl JaqExecutor {
    /// Create a new JaqExecutor.
    pub fn new() -> Self {
        Self {}
    }

    /// Execute a jaq filter on JSON input.
    ///
    /// # Arguments
    ///
    /// * `filter` - The jaq filter expression (e.g., ".files[0].path")
    /// * `input` - The input JSON value to filter
    /// * `string_vars` - String variables for $var substitution (--arg key value)
    /// * `json_vars` - JSON variables for $var substitution (--argjson key value)
    ///
    /// # Returns
    ///
    /// A vector of output values (jaq filters can produce multiple outputs)
    pub fn execute(
        &self,
        filter: &str,
        input: &Value,
        string_vars: &[(String, String)],
        json_vars: &[(String, String)],
    ) -> Result<Vec<Value>, String> {
        use jaq_core::{compile, load};
        use std::path::PathBuf;

        // Collect variable names for the compiler
        let mut var_names = Vec::new();
        for (name, _) in string_vars {
            var_names.push(format!("${}", name));
        }
        for (name, _) in json_vars {
            var_names.push(format!("${}", name));
        }

        // Create arena and loader
        let arena = load::Arena::default();
        let defs = jaq_core::defs()
            .chain(jaq_std::defs())
            .chain(jaq_json::defs());
        let loader = load::Loader::new(defs);

        // Load the filter as a module
        let path = PathBuf::from("<inline>");
        let file = load::File { path, code: filter };

        let modules = loader
            .load(&arena, file)
            .map_err(|errs| format_load_errors(&errs))?;

        // Compile the filter
        // Use the standard library functions from jaq-std and jaq-json
        let compiler = compile::Compiler::default()
            .with_funs(
                jaq_core::funs()
                    .chain(jaq_std::funs())
                    .chain(jaq_json::funs()),
            )
            .with_global_vars(var_names.iter().map(|s| s.as_str()));

        let compiled_filter = compiler
            .compile(modules)
            .map_err(|errs| format_compile_errors(&errs))?;

        // Convert serde_json::Value to jaq_json::Val
        let jaq_input = serde_json_to_jaq(input);

        // Prepare variable values
        let mut var_vals = Vec::new();

        // Add string variables as Val::Str
        for (_, value) in string_vars {
            var_vals.push(jaq_json::Val::from(value.clone()));
        }

        // Add JSON variables
        for (_, value) in json_vars {
            let parsed: Value = serde_json::from_str(value)
                .map_err(|e| format!("Invalid JSON in variable: {}", e))?;
            var_vals.push(serde_json_to_jaq(&parsed));
        }

        use jaq_core::{Ctx, Vars, data, unwrap_valr};

        let ctx =
            Ctx::<data::JustLut<jaq_json::Val>>::new(&compiled_filter.lut, Vars::new(var_vals));

        let mut results = Vec::new();
        for result in compiled_filter.id.run((ctx, jaq_input)).map(unwrap_valr) {
            let val = result.map_err(|e| format!("Filter execution error: {}", e))?;
            results.push(jaq_to_serde_json(&val));
        }

        Ok(results)
    }
}

impl Default for JaqExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert serde_json::Value to jaq_json::Val
fn serde_json_to_jaq(value: &Value) -> jaq_json::Val {
    match value {
        Value::Null => jaq_json::Val::Null,
        Value::Bool(b) => jaq_json::Val::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                // Try to fit into isize
                if let Ok(ival) = isize::try_from(i) {
                    jaq_json::Val::Num(jaq_json::Num::Int(ival))
                } else {
                    jaq_json::Val::Num(jaq_json::Num::Dec(Rc::new(i.to_string())))
                }
            } else if let Some(f) = n.as_f64() {
                jaq_json::Val::Num(jaq_json::Num::Float(f))
            } else {
                // Fallback for u64
                let u = n.as_u64().unwrap_or(0);
                if let Ok(ival) = isize::try_from(u) {
                    jaq_json::Val::Num(jaq_json::Num::Int(ival))
                } else {
                    jaq_json::Val::Num(jaq_json::Num::Dec(Rc::new(u.to_string())))
                }
            }
        }
        Value::String(s) => jaq_json::Val::from(s.clone()),
        Value::Array(arr) => {
            let items: Vec<_> = arr.iter().map(serde_json_to_jaq).collect();
            jaq_json::Val::Arr(Rc::new(items))
        }
        Value::Object(obj) => {
            // Build the map step by step
            let pairs: Vec<(jaq_json::Val, jaq_json::Val)> = obj
                .iter()
                .map(|(k, v)| (jaq_json::Val::from(k.clone()), serde_json_to_jaq(v)))
                .collect();

            // Create the internal map structure that jaq-json uses
            // The map uses IndexMap with foldhash
            let map = pairs.into_iter().collect();
            jaq_json::Val::Obj(Rc::new(map))
        }
    }
}

/// Convert jaq_json::Val to serde_json::Value
fn jaq_to_serde_json(val: &jaq_json::Val) -> Value {
    match val {
        jaq_json::Val::Null => Value::Null,
        jaq_json::Val::Bool(b) => Value::Bool(*b),
        jaq_json::Val::Num(n) => jaq_num_to_serde_json(n),
        jaq_json::Val::TStr(s) | jaq_json::Val::BStr(s) => {
            Value::String(String::from_utf8_lossy(s).into_owned())
        }
        jaq_json::Val::Arr(arr) => Value::Array(arr.iter().map(jaq_to_serde_json).collect()),
        jaq_json::Val::Obj(obj) => {
            let map: serde_json::Map<String, Value> = obj
                .iter()
                .map(|(k, v)| (jaq_key_to_string(k), jaq_to_serde_json(v)))
                .collect();
            Value::Object(map)
        }
    }
}

fn jaq_num_to_serde_json(n: &jaq_json::Num) -> Value {
    match n {
        jaq_json::Num::Int(i) => Value::Number((*i as i64).into()),
        jaq_json::Num::BigInt(i) => i
            .to_string()
            .parse::<i64>()
            .map(|i| Value::Number(i.into()))
            .unwrap_or_else(|_| Value::String(i.to_string())),
        jaq_json::Num::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        jaq_json::Num::Dec(s) => {
            if let Ok(i) = s.parse::<i64>() {
                Value::Number(i.into())
            } else if let Ok(f) = s.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            } else {
                Value::String(s.to_string())
            }
        }
    }
}

fn jaq_key_to_string(key: &jaq_json::Val) -> String {
    match jaq_to_serde_json(key) {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Format output based on options
pub fn format_output(val: &Value, raw: bool, compact: bool) -> String {
    if raw {
        // Raw output mode: if string, print without quotes
        match val {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => {
                if compact {
                    serde_json::to_string(val).unwrap_or_default()
                } else {
                    serde_json::to_string_pretty(val).unwrap_or_default()
                }
            }
        }
    } else {
        // JSON output mode
        if compact {
            serde_json::to_string(val).unwrap_or_default()
        } else {
            serde_json::to_string_pretty(val).unwrap_or_default()
        }
    }
}

/// Format load errors into a human-readable string
fn format_load_errors(errs: &jaq_core::load::Errors<&str, std::path::PathBuf>) -> String {
    // Errors is a Vec<(File, Error)>
    errs.iter()
        .map(|(file, error)| format!("In {}:\n  - {:?}", file.path.display(), error))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format compile errors into a human-readable string
fn format_compile_errors(errs: &jaq_core::compile::Errors<&str, std::path::PathBuf>) -> String {
    // compile::Errors is an alias for load::Errors with different error type
    errs.iter()
        .map(|(file, errors)| {
            let error_strs: Vec<_> = errors.iter().map(|e| format!("  - {}", e.0)).collect();
            format!("In {}:\n{}", file.path.display(), error_strs.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_filter() {
        let executor = JaqExecutor::new();
        let input = json!({"name": "test", "value": 42});

        let result = executor.execute(".name", &input, &[], &[]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], json!("test"));
    }

    #[test]
    fn test_array_filter() {
        let executor = JaqExecutor::new();
        let input = json!({"items": [1, 2, 3, 4, 5]});

        let result = executor
            .execute(".items | map(. * 2)", &input, &[], &[])
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], json!([2, 4, 6, 8, 10]));
    }

    #[test]
    fn test_identity_filter() {
        let executor = JaqExecutor::new();
        let input = json!({"test": "data"});

        let result = executor.execute(".", &input, &[], &[]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], input);
    }

    #[test]
    fn test_string_variables() {
        let executor = JaqExecutor::new();
        let input = json!({});

        let vars = vec![("name".to_string(), "Alice".to_string())];
        let result = executor.execute("$name", &input, &vars, &[]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], json!("Alice"));
    }

    #[test]
    fn test_format_output_raw() {
        let val = json!("hello");
        assert_eq!(format_output(&val, true, false), "hello");

        let val = json!(42);
        assert_eq!(format_output(&val, true, false), "42");
    }

    #[test]
    fn test_format_output_compact() {
        let val = json!({"a": 1, "b": 2});
        let output = format_output(&val, false, true);
        assert!(!output.contains('\n'));
        assert!(output.contains("\"a\":1"));
    }
}
