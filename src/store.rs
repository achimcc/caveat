//! Where the caveats are, and all of them.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::entry::{self, Entry};

/// The directory a repository keeps its caveats in.
pub const DIR: &str = "caveats";

#[derive(Deserialize)]
struct Config {
    dir: PathBuf,
}

/// Upwards from `cwd`, like lotse looks for its `lotse.toml`: a worktree brings
/// its own version. The config is the fallback for repositories without any —
/// `None` when there is no config home to look in (`config_home` gave none).
pub fn find_dir(cwd: &Path, config_home: Option<&Path>) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        let candidate = dir.join(DIR);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let raw = std::fs::read_to_string(config_home?.join("caveat/config.toml")).ok()?;
    let config: Config = toml::from_str(&raw).ok()?;
    config.dir.is_dir().then_some(config.dir)
}

/// Every `*.md` of the directory, sorted by name. A file that does not parse
/// does not take the others down with it.
#[allow(clippy::type_complexity)]
pub fn load(dir: &Path) -> Result<(Vec<Entry>, Vec<(PathBuf, anyhow::Error)>)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    let mut entries = Vec::new();
    let mut broken = Vec::new();
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .context("reading the file")
            .and_then(|raw| entry::parse(&path, &raw));
        match parsed {
            Ok(entry) => entries.push(entry),
            Err(error) => broken.push((path, error)),
        }
    }
    Ok((entries, broken))
}

/// `var`, but only if it is non-empty and absolute. An `XDG_*` variable
/// that is SET but EMPTY, or a relative one, must not silently produce a
/// relative state or config path — the hook would then create
/// `.local/state/caveat/` inside whatever directory it runs in.
fn absolute_dir(var: Option<&OsStr>) -> Option<PathBuf> {
    let var = var.filter(|v| !v.is_empty())?;
    let path = PathBuf::from(var);
    path.is_absolute().then_some(path)
}

/// Pure: `XDG_CONFIG_HOME` if it gives an absolute path, else `$HOME/.config`
/// if `HOME` gives one, else `None`. Takes both variables as arguments so a
/// test does not have to mutate the process environment (tests run in
/// parallel).
fn config_home_from(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    absolute_dir(xdg_config_home).or_else(|| absolute_dir(home).map(|h| h.join(".config")))
}

/// Pure, same shape as `config_home_from`: `XDG_STATE_HOME/caveat` or
/// `$HOME/.local/state/caveat`, or `None`.
fn state_dir_from(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let base = absolute_dir(xdg_state_home)
        .or_else(|| absolute_dir(home).map(|h| h.join(".local/state")))?;
    Some(base.join("caveat"))
}

/// `None` when neither `XDG_CONFIG_HOME` nor `HOME` gives an absolute path:
/// callers then have no fallback to look in (`find_dir` takes `Option`).
pub fn config_home() -> Option<PathBuf> {
    config_home_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// `None` when neither `XDG_STATE_HOME` nor `HOME` gives an absolute path:
/// callers then keep no log and no per-session `seen` file
/// (`Seen::memory()`, and `main::log` does nothing).
pub fn state_dir() -> Option<PathBuf> {
    state_dir_from(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const GOOD: &str = "---\ntitle: T\nline: L\n---\nB\n";

    #[test]
    fn the_directory_is_found_upwards_from_the_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("repo/caveats")).unwrap();
        fs::create_dir_all(tmp.path().join("repo/a/b")).unwrap();
        let found = find_dir(&tmp.path().join("repo/a/b"), Some(&tmp.path().join("cfg")));
        assert_eq!(found, Some(tmp.path().join("repo/caveats")));
    }

    #[test]
    fn no_config_home_is_no_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("other")).unwrap();
        assert_eq!(find_dir(&tmp.path().join("other"), None), None);
    }

    #[test]
    fn the_config_is_the_fallback_and_a_dead_path_in_it_is_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("elsewhere/caveats");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(tmp.path().join("cfg/caveat")).unwrap();
        fs::create_dir_all(tmp.path().join("other")).unwrap();
        let cfg = tmp.path().join("cfg/caveat/config.toml");
        fs::write(&cfg, format!("dir = \"{}\"\n", data.display())).unwrap();
        assert_eq!(
            find_dir(&tmp.path().join("other"), Some(&tmp.path().join("cfg"))),
            Some(data)
        );
        fs::write(&cfg, "dir = \"/does/not/exist\"\n").unwrap();
        assert_eq!(
            find_dir(&tmp.path().join("other"), Some(&tmp.path().join("cfg"))),
            None
        );
    }

    #[test]
    fn loading_sorts_keeps_broken_files_apart_and_ignores_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("b.md"), GOOD).unwrap();
        fs::write(tmp.path().join("a.md"), GOOD).unwrap();
        fs::write(tmp.path().join("broken.md"), "no frontmatter").unwrap();
        fs::write(tmp.path().join("harmless.txt"), "git status\n").unwrap();
        let (entries, broken) = load(tmp.path()).unwrap();
        let slugs: Vec<&str> = entries.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, ["a", "b"]);
        assert_eq!(broken.len(), 1);
        assert!(broken[0].0.ends_with("broken.md"));
    }

    /// A directory named `x.md` is not a caveat: the `*.md` filter alone
    /// does not say "a file", and `entry::parse` reading it as one would
    /// add a `broken` line to every hook call.
    #[test]
    fn a_directory_named_like_a_caveat_is_neither_an_entry_nor_a_broken_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.md"), GOOD).unwrap();
        fs::create_dir(tmp.path().join("dir.md")).unwrap();
        let (entries, broken) = load(tmp.path()).unwrap();
        let slugs: Vec<&str> = entries.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, ["a"]);
        assert!(broken.is_empty());
    }

    #[test]
    fn the_xdg_variable_wins_when_it_is_absolute() {
        assert_eq!(
            config_home_from(Some(OsStr::new("/xdg/cfg")), Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/xdg/cfg"))
        );
        assert_eq!(
            state_dir_from(Some(OsStr::new("/xdg/state")), Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/xdg/state/caveat"))
        );
    }

    #[test]
    fn an_empty_xdg_variable_falls_back_to_home() {
        assert_eq!(
            config_home_from(Some(OsStr::new("")), Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/home/x/.config"))
        );
        assert_eq!(
            state_dir_from(Some(OsStr::new("")), Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/home/x/.local/state/caveat"))
        );
    }

    #[test]
    fn a_missing_xdg_variable_falls_back_to_home_too() {
        assert_eq!(
            config_home_from(None, Some(OsStr::new("/home/x"))),
            Some(PathBuf::from("/home/x/.config"))
        );
    }

    #[test]
    fn neither_the_xdg_variable_nor_home_gives_a_relative_path() {
        // Empty XDG variable, no HOME at all: no fallback exists, so the
        // result is None — never a relative path built from nothing.
        assert_eq!(config_home_from(Some(OsStr::new("")), None), None);
        assert_eq!(state_dir_from(Some(OsStr::new("")), None), None);
        // Empty XDG variable, HOME set but relative: still no absolute
        // fallback.
        assert_eq!(
            config_home_from(Some(OsStr::new("")), Some(OsStr::new("relative/home"))),
            None
        );
        // XDG variable itself relative: not used as-is either.
        assert_eq!(
            state_dir_from(
                Some(OsStr::new("relative/xdg")),
                Some(OsStr::new("/home/x"))
            ),
            Some(PathBuf::from("/home/x/.local/state/caveat"))
        );
    }
}
