//! Where the caveats are, and all of them.

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
/// its own version. The config is the fallback for repositories without any.
pub fn find_dir(cwd: &Path, config_home: &Path) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        let candidate = dir.join(DIR);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let raw = std::fs::read_to_string(config_home.join("caveat/config.toml")).ok()?;
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

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state"))
        .join("caveat")
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
        let found = find_dir(&tmp.path().join("repo/a/b"), &tmp.path().join("cfg"));
        assert_eq!(found, Some(tmp.path().join("repo/caveats")));
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
            find_dir(&tmp.path().join("other"), &tmp.path().join("cfg")),
            Some(data)
        );
        fs::write(&cfg, "dir = \"/does/not/exist\"\n").unwrap();
        assert_eq!(
            find_dir(&tmp.path().join("other"), &tmp.path().join("cfg")),
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
}
