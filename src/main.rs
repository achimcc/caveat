use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use caveat::seen::Seen;
use caveat::{check, hook, index, search, store};
use serde_json::Value;

const USAGE: &str = "usage: caveat hook claude | caveat check [--dir DIR] [--index FILE] | \
caveat search [--dir DIR] TEXT… | caveat gen --index FILE | caveat --version | caveat --help";

/// Bytes past which `log` rotates `hook.log` to `hook.log.1` before
/// appending: 1 MiB.
const HOOK_LOG_ROTATE_AT: u64 = 1 << 20;

fn log(line: &str) {
    // Neither `XDG_STATE_HOME` nor `HOME` gives an absolute path: there is
    // nowhere safe to write, and staying quiet beats creating
    // `.local/state/caveat/` inside whatever directory this runs in.
    let Some(dir) = store::state_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("hook.log");
    // Keep the file from growing without bound: past the threshold, its
    // current content becomes `hook.log.1` (overwriting an older one) and
    // appending below starts a fresh file. Any error here — the file is
    // gone, the rename fails — is ignored and appending proceeds regardless.
    if std::fs::metadata(&path)
        .map(|meta| meta.len() > HOOK_LOG_ROTATE_AT)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("hook.log.1"));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        // One `write_all` of one preformatted buffer: with `O_APPEND`, a single
        // `write(2)` is atomic against other processes appending to the same
        // file; several `write!` calls for one line are not (measured with
        // strace: four writes for one line, interleavable by a concurrent
        // session's hook run).
        let _ = file.write_all(format!("{now} {line}\n").as_bytes());
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
    let Some(dir) = store::find_dir(&cwd, store::config_home().as_deref()) else {
        return Ok(());
    };
    let (entries, broken) = store::load(&dir)?;
    for (path, error) in &broken {
        log(&format!("broken {}: {error:#}", path.display()));
    }
    // No state dir → no `seen` file either: kept in memory only, so a
    // caveat can still be shown, just not remembered across hook calls.
    let mut seen = match store::state_dir() {
        Some(dir) => Seen::open(
            &dir,
            event["session_id"].as_str().unwrap_or("unknown"),
            event["agent_id"].as_str(),
        ),
        None => Seen::memory(),
    };
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
        None => store::find_dir(&std::env::current_dir()?, store::config_home().as_deref())
            .context("no `caveats` directory above here, and none in the config; use --dir"),
    }
}

fn run_check(dir: Option<PathBuf>, index_file: Option<PathBuf>) -> Result<ExitCode> {
    let dir = dir_or_found(dir)?;
    let mut findings = check::check(&dir)?;
    if let Some(index_file) = &index_file {
        // A marker error here is a tool error (`?`, exit 2), not a
        // finding: `check::check_index` already draws that line.
        if let Some(finding) = check::check_index(&dir, index_file)? {
            findings.push(finding);
        }
    }
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

/// `caveat gen --index FILE`: splices a freshly rendered index into `FILE`
/// between its markers, writing back only when that changes the text.
fn run_gen(dir: Option<PathBuf>, index_file: PathBuf) -> Result<ExitCode> {
    let dir = dir_or_found(dir)?;
    let (entries, broken) = store::load(&dir)?;
    for (path, error) in &broken {
        eprintln!("caveat: {}: {error:#}", path.display());
    }
    let text = std::fs::read_to_string(&index_file)
        .with_context(|| format!("reading {}", index_file.display()))?;
    let block = index::render(&entries, &index_file);
    let spliced = index::splice(&text, &block)?;
    if spliced != text {
        std::fs::write(&index_file, &spliced)
            .with_context(|| format!("writing {}", index_file.display()))?;
    }
    println!(
        "caveat: index in {}: {} entries",
        index_file.display(),
        entries.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn run_search(dir: Option<PathBuf>, query: &str) -> Result<ExitCode> {
    let (entries, broken) = store::load(&dir_or_found(dir)?)?;
    for (path, error) in &broken {
        eprintln!("caveat: {}: {error:#}", path.display());
    }
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
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version"] | ["-V"] => {
            println!("caveat {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        ["--help"] | ["-h"] | ["help"] => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let mut dir = None;
    if let Some(at) = args.iter().position(|a| a == "--dir") {
        if at + 1 >= args.len() {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        dir = Some(PathBuf::from(args.remove(at + 1)));
        args.remove(at);
    }
    let mut index_file = None;
    if let Some(at) = args.iter().position(|a| a == "--index") {
        if at + 1 >= args.len() {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        index_file = Some(PathBuf::from(args.remove(at + 1)));
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
        ["check"] => run_check(dir, index_file),
        ["gen"] => match index_file {
            Some(index_file) => run_gen(dir, index_file),
            None => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        },
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
