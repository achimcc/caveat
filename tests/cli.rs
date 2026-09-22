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

/// The recorded event, with its cwd pointed at the test repository and,
/// optionally, its own session id (two different sessions each log a hit).
fn recorded(tmp: &Path) -> String {
    recorded_for_session(tmp, "3ac76f8c-38b5-45ab-b497-21ef0a352f7d")
}

fn recorded_for_session(tmp: &Path, session_id: &str) -> String {
    let raw = std::fs::read_to_string("tests/recorded/post_tool_use.json").unwrap();
    let mut event: serde_json::Value = serde_json::from_str(&raw).unwrap();
    event["cwd"] = serde_json::Value::String(tmp.join("repo").display().to_string());
    event["session_id"] = serde_json::Value::String(session_id.to_string());
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
    // A second, distinct session logs a second hit line: two log-producing
    // hook runs, so the file has two lines to pin the content of.
    let third = run(
        &["hook", "claude"],
        &recorded_for_session(tmp.path(), "a-different-session"),
        &state,
        &config,
    );
    assert!(third.status.success());
    let log = std::fs::read_to_string(state.join("caveat/hook.log")).unwrap();
    assert!(log.contains("hit PostToolUse probe"));
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "one line per hook run that hit: {log:?}");
    for line in &lines {
        assert!(
            line.split_once(' ').is_some_and(|(ts, rest)| {
                !ts.is_empty()
                    && ts.chars().all(|c| c.is_ascii_digit())
                    && (rest.starts_with("hit ")
                        || rest.starts_with("error ")
                        || rest.starts_with("broken "))
            }),
            "line does not match `^\\d+ (hit|error|broken) `: {line:?}"
        );
    }
}

#[test]
fn empty_xdg_variables_and_no_home_create_no_relative_local_directory() {
    let tmp = repo();
    // A caveat that DOES fire, so the hook reaches `log()` and `Seen::open`
    // — where a relative state path would show up as `.local/` created
    // relative to the process's cwd, which we set to a fresh directory.
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_caveat"))
        .args(["hook", "claude"])
        .env("XDG_STATE_HOME", "")
        .env("XDG_CONFIG_HOME", "")
        .env_remove("HOME")
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
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
    // Neither `.local/state/caveat` (the unset-variable shape) nor a bare
    // `caveat/` (what `XDG_STATE_HOME=""` produces when an empty variable
    // is treated as set) may appear relative to the process's cwd.
    let created: Vec<_> = std::fs::read_dir(&cwd)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(
        created.is_empty(),
        "no directory should have been created relative to cwd, found: {created:?}"
    );
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
fn gen_writes_the_index_and_check_index_sees_it_go_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo/caveats");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.md"),
        "---\ntitle: A title\nline: a line\noutput:\n  - text: hit-a\n    hits: hit-a\n    misses: x\n---\nBODY-A\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.md"),
        "---\ntitle: B title\nline: b line\noutput:\n  - text: hit-b\n    hits: hit-b\n    misses: y\n---\nBODY-B\n",
    )
    .unwrap();
    std::fs::write(dir.join("harmless.txt"), "git status\n").unwrap();
    let claude = tmp.path().join("repo/CLAUDE.md");
    std::fs::write(
        &claude,
        "# CLAUDE.md\n\n<!-- caveat:index -->\nalt\n<!-- /caveat:index -->\n\nmore text\n",
    )
    .unwrap();
    let (state, config) = (tmp.path().join("state"), tmp.path().join("config"));
    let dir_arg = dir.to_str().unwrap();
    let claude_arg = claude.to_str().unwrap();

    let generated = run(
        &["gen", "--dir", dir_arg, "--index", claude_arg],
        "",
        &state,
        &config,
    );
    assert_eq!(generated.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&generated.stdout).contains("2 entries"));
    let after_gen = std::fs::read_to_string(&claude).unwrap();
    assert_eq!(after_gen.matches("- **").count(), 2);
    assert!(!after_gen.contains("alt"));
    assert!(after_gen.starts_with("# CLAUDE.md\n\n"));
    assert!(after_gen.ends_with("\nmore text\n"));

    let check_ok = run(
        &["check", "--dir", dir_arg, "--index", claude_arg],
        "",
        &state,
        &config,
    );
    assert_eq!(check_ok.status.code(), Some(0));

    // Bend one index line by hand: `check --index` must notice.
    let bent = after_gen.replace("A title", "A title, bent");
    std::fs::write(&claude, &bent).unwrap();
    let check_stale = run(
        &["check", "--dir", dir_arg, "--index", claude_arg],
        "",
        &state,
        &config,
    );
    assert_eq!(check_stale.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&check_stale.stdout).contains("index out of date"),
        "{:?}",
        String::from_utf8_lossy(&check_stale.stdout)
    );

    // Remove the markers entirely: a tool error, not a finding.
    std::fs::write(&claude, "# CLAUDE.md\n\nno markers here\n").unwrap();
    let check_broken = run(
        &["check", "--dir", dir_arg, "--index", claude_arg],
        "",
        &state,
        &config,
    );
    assert_eq!(check_broken.status.code(), Some(2));
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
fn search_reports_a_broken_file_on_stderr() {
    let tmp = repo();
    let (state, config) = (tmp.path().join("state"), tmp.path().join("config"));
    let dir = tmp.path().join("repo/caveats");
    let dir_arg = dir.to_str().unwrap();
    std::fs::write(dir.join("broken.md"), "no frontmatter here").unwrap();
    let found = run(
        &["search", "--dir", dir_arg, "spike-erfolg"],
        "",
        &state,
        &config,
    );
    assert_eq!(found.status.code(), Some(0));
    let err = String::from_utf8_lossy(&found.stderr);
    assert!(
        err.contains("broken.md"),
        "stderr should name the broken file: {err:?}"
    );
    assert!(
        err.starts_with("caveat: "),
        "stderr should follow `caveat: <path>: <error>`: {err:?}"
    );
}

#[test]
fn version_prints_the_crate_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_caveat"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("caveat {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_goes_to_stdout_with_exit_zero() {
    for flag in ["--help", "-h", "help"] {
        let out = Command::new(env!("CARGO_BIN_EXE_caveat"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(out.status.success(), "{flag}");
        assert!(
            String::from_utf8_lossy(&out.stdout).starts_with("usage: caveat"),
            "{flag}"
        );
    }
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
