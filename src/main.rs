use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use caveat::seen::Seen;
use caveat::{check, hook, search, store};
use serde_json::Value;

const USAGE: &str =
    "usage: caveat hook claude | caveat check [--dir DIR] | caveat search [--dir DIR] TEXT…";

fn log(line: &str) {
    let dir = store::state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("hook.log"))
    {
        let _ = writeln!(file, "{now} {line}");
    }
}

/// The hook must never hold a session up: whatever goes wrong is a log line.
fn run_hook() -> Result<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let event: Value = serde_json::from_str(&raw).context("stdin is not JSON")?;
    let cwd = event["cwd"]
        .as_str()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .context("no cwd")?;
    let Some(dir) = store::find_dir(&cwd, &store::config_home()) else {
        return Ok(());
    };
    let (entries, broken) = store::load(&dir)?;
    for (path, error) in &broken {
        log(&format!("broken {}: {error:#}", path.display()));
    }
    let mut seen = Seen::open(
        &store::state_dir(),
        event["session_id"].as_str().unwrap_or("unknown"),
        event["agent_id"].as_str(),
    );
    if let Some(reply) = hook::respond(&entries, &event, &mut seen) {
        let name = event["hook_event_name"].as_str().unwrap_or("?");
        for slug in &reply.slugs {
            log(&format!("hit {name} {slug}"));
        }
        // Not println!: it panics when stdout is gone (a closed pipe, a full disk),
        // and a panic is exit 101 — the one thing this hook must never do.
        let mut out = std::io::stdout().lock();
        writeln!(out, "{}", reply.json).context("writing the reply")?;
    }
    Ok(())
}

fn dir_or_found(dir: Option<PathBuf>) -> Result<PathBuf> {
    match dir {
        Some(dir) => Ok(dir),
        None => store::find_dir(&std::env::current_dir()?, &store::config_home())
            .context("no `caveats` directory above here, and none in the config; use --dir"),
    }
}

fn run_check(dir: Option<PathBuf>) -> Result<ExitCode> {
    let dir = dir_or_found(dir)?;
    let findings = check::check(&dir)?;
    for f in &findings {
        println!("{}: {}", f.path.display(), f.message);
    }
    if findings.is_empty() {
        eprintln!(
            "caveat: every trigger in {} holds; the recorded hook events still reply",
            dir.display()
        );
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

fn run_search(dir: Option<PathBuf>, query: &str) -> Result<ExitCode> {
    let (entries, _) = store::load(&dir_or_found(dir)?)?;
    let found = search::search(&entries, query);
    for e in &found {
        println!("{}\n  {}\n  {}", e.title, e.path.display(), e.line);
    }
    Ok(if found.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = None;
    if let Some(at) = args.iter().position(|a| a == "--dir") {
        if at + 1 >= args.len() {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        dir = Some(PathBuf::from(args.remove(at + 1)));
        args.remove(at);
    }
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["hook", "claude"] => {
            if let Err(error) = run_hook() {
                log(&format!("error {error:#}"));
            }
            return ExitCode::SUCCESS;
        }
        ["check"] => run_check(dir),
        ["search", query @ ..] if !query.is_empty() => run_search(dir, &query.join(" ")),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    result.unwrap_or_else(|error| {
        eprintln!("caveat: {error:#}");
        ExitCode::from(2)
    })
}
