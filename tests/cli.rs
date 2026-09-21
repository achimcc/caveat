use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const CAVEAT: &str = "---\ntitle: Probe\nline: l\noutput:\n  - text: spike-erfolg\n    hits: spike-erfolg\n    misses: x\n---\nBODY-OF-THE-PROBE\n";

fn run(args: &[&str], stdin: &str, state: &Path, config: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_caveat"))
        .args(args)
        .env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("repo/caveats")).unwrap();
    std::fs::write(tmp.path().join("repo/caveats/probe.md"), CAVEAT).unwrap();
    std::fs::write(tmp.path().join("repo/caveats/harmless.txt"), "git status\n").unwrap();
    tmp
}

/// The recorded event, with its cwd pointed at the test repository.
fn recorded(tmp: &Path) -> String {
    let raw = std::fs::read_to_string("tests/recorded/post_tool_use.json").unwrap();
    let mut event: serde_json::Value = serde_json::from_str(&raw).unwrap();
    event["cwd"] = serde_json::Value::String(tmp.join("repo").display().to_string());
    event.to_string()
}

#[test]
fn the_hook_shows_the_caveat_once_and_logs_the_hit() {
    let tmp = repo();
    let (state, config) = (tmp.path().join("state"), tmp.path().join("config"));
    let first = run(&["hook", "claude"], &recorded(tmp.path()), &state, &config);
    assert!(first.status.success());
    let reply: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let text = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("BODY-OF-THE-PROBE"));
    let second = run(&["hook", "claude"], &recorded(tmp.path()), &state, &config);
    assert!(second.status.success() && second.stdout.is_empty());
    let log = std::fs::read_to_string(state.join("caveat/hook.log")).unwrap();
    assert!(log.contains("hit PostToolUse probe"));
}

#[test]
fn a_closed_stdout_is_still_exit_zero() {
    let tmp = repo();
    let (state, config) = (tmp.path().join("state"), tmp.path().join("config"));
    // Every write to /dev/full fails with ENOSPC: the disk-full case of the finding.
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_caveat"))
        .args(["hook", "claude"])
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", &config)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(full))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(recorded(tmp.path()).as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let log = std::fs::read_to_string(state.join("caveat/hook.log")).unwrap();
    assert!(log.contains("error"));
}

#[test]
fn a_broken_stdin_is_silent_exits_zero_and_leaves_a_log_line() {
    let tmp = repo();
    let (state, config) = (tmp.path().join("state"), tmp.path().join("config"));
    let out = run(&["hook", "claude"], "kaputt", &state, &config);
    assert!(out.status.success() && out.stdout.is_empty());
    let log = std::fs::read_to_string(state.join("caveat/hook.log")).unwrap();
    assert!(log.contains("error"));
}

#[test]
fn check_exits_zero_one_and_two() {
    let tmp = repo();
    let (state, config) = (tmp.path().join("state"), tmp.path().join("config"));
    let dir = tmp.path().join("repo/caveats");
    let dir_arg = dir.to_str().unwrap();
    assert_eq!(
        run(&["check", "--dir", dir_arg], "", &state, &config)
            .status
            .code(),
        Some(0)
    );
    std::fs::write(
        dir.join("probe.md"),
        CAVEAT.replace("hits: spike-erfolg", "hits: something-else"),
    )
    .unwrap();
    let red = run(&["check", "--dir", dir_arg], "", &state, &config);
    assert_eq!(red.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&red.stdout).contains("probe.md"));
    std::fs::remove_file(dir.join("probe.md")).unwrap();
    assert_eq!(
        run(&["check", "--dir", dir_arg], "", &state, &config)
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn search_finds_by_a_pasted_message_and_exits_one_on_nothing() {
    let tmp = repo();
    let (state, config) = (tmp.path().join("state"), tmp.path().join("config"));
    let dir = tmp.path().join("repo/caveats");
    let dir_arg = dir.to_str().unwrap();
    let found = run(
        &["search", "--dir", dir_arg, "output:", "spike-erfolg"],
        "",
        &state,
        &config,
    );
    assert_eq!(found.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&found.stdout).contains("Probe"));
    assert_eq!(
        run(&["search", "--dir", dir_arg, "zebra"], "", &state, &config)
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn an_unknown_command_is_usage_and_exit_two() {
    let tmp = repo();
    let out = run(
        &["frobnicate"],
        "",
        &tmp.path().join("s"),
        &tmp.path().join("c"),
    );
    assert_eq!(out.status.code(), Some(2));
}
