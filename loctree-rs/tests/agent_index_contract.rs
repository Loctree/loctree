use assert_cmd::cargo::cargo_bin_cmd;
use std::process::Command;

fn loct() -> assert_cmd::Command {
    cargo_bin_cmd!("loct")
}

fn agent_index_url() -> &'static str {
    "https://loct.io/api/agent/index.json"
}

fn fetch_agent_index_json() -> serde_json::Value {
    let output = Command::new("curl")
        .args(["-fsSL", agent_index_url()])
        .output()
        .expect("run curl for loct.io agent index");
    assert!(
        output.status.success(),
        "failed to fetch {} (status: {}):\n{}",
        agent_index_url(),
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let text = String::from_utf8(output.stdout).expect("decode loct.io agent index as utf-8");
    serde_json::from_str(&text).expect("parse https://loct.io/api/agent/index.json")
}

fn help_args_for_agent_cmd(cmd: &str) -> Vec<String> {
    let trimmed = cmd.trim();
    assert!(
        trimmed.starts_with("loct"),
        "agent index command must start with 'loct': {trimmed}"
    );

    let rest = trimmed
        .strip_prefix("loct")
        .unwrap_or(trimmed)
        .trim()
        .to_string();

    if rest.is_empty() {
        return vec!["--help".to_string()];
    }

    if rest.starts_with("--for-ai") {
        return vec!["--for-ai".to_string(), "--help".to_string()];
    }

    if rest.starts_with("--watch") {
        return vec!["--watch".to_string(), "--help".to_string()];
    }

    // JQ mode: loct '.metadata' (or other quoted jq filters)
    if let Some(quote) = rest.chars().next()
        && (quote == '\'' || quote == '"')
    {
        let mut closing = None;
        for (idx, ch) in rest[1..].char_indices() {
            if ch == quote {
                closing = Some(idx + 1);
                break;
            }
        }
        let end = closing.expect("unterminated quoted jq filter in agent index cmd");
        let filter = &rest[1..end];
        return vec![filter.to_string(), "--help".to_string()];
    }

    let subcommand = rest
        .split_whitespace()
        .next()
        .expect("expected subcommand after 'loct'");
    vec![subcommand.to_string(), "--help".to_string()]
}

#[test]
fn agent_index_commands_have_help() {
    let json = fetch_agent_index_json();

    let commands = json
        .get("commands")
        .and_then(|v| v.as_object())
        .expect("https://loct.io/api/agent/index.json must have a top-level 'commands' object");

    for (name, entry) in commands {
        let cmd_str = entry
            .get("cmd")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("commands.{name}.cmd must be a string"));

        let help_args = help_args_for_agent_cmd(cmd_str);
        let mut cmd = loct();
        cmd.args(&help_args);

        let output = cmd
            .output()
            .unwrap_or_else(|e| panic!("failed to run loct for commands.{name}: {e}"));

        if !output.status.success() {
            panic!(
                "Agent index command help failed:\n- key: commands.{name}\n- cmd: {cmd_str}\n- help invocation: loct {}\n- status: {}\n- stdout:\n{}\n- stderr:\n{}",
                help_args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
}
