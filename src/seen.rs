//! Which caveats a session has been shown already.
//!
//! The key is the session AND the agent: a subagent starts with a fresh
//! context and has not read what its parent was shown.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Seen {
    path: Option<PathBuf>,
    slugs: BTreeSet<String>,
}

fn safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Seen {
    pub fn memory() -> Seen {
        Seen {
            path: None,
            slugs: BTreeSet::new(),
        }
    }

    pub fn open(state_dir: &Path, session: &str, agent: Option<&str>) -> Seen {
        let name = format!("{}--{}.txt", safe(session), safe(agent.unwrap_or("main")));
        let path = state_dir.join("seen").join(name);
        let slugs = std::fs::read_to_string(&path)
            .map(|raw| raw.lines().map(String::from).collect())
            .unwrap_or_default();
        Seen {
            path: Some(path),
            slugs,
        }
    }

    pub fn contains(&self, slug: &str) -> bool {
        self.slugs.contains(slug)
    }

    pub fn add(&mut self, slug: &str) {
        if !self.slugs.insert(slug.to_string()) {
            return;
        }
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            // One `write_all` of one preformatted buffer, for the same reason
            // as `main::log`: `O_APPEND` makes a single `write(2)` atomic,
            // not a line built from several of them.
            let _ = file.write_all(format!("{slug}\n").as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_added_is_still_there_in_the_next_process() {
        let tmp = tempfile::tempdir().unwrap();
        let mut first = Seen::open(tmp.path(), "s1", None);
        assert!(!first.contains("a"));
        first.add("a");
        assert!(first.contains("a"));
        assert!(Seen::open(tmp.path(), "s1", None).contains("a"));
    }

    #[test]
    fn another_session_and_a_subagent_start_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        Seen::open(tmp.path(), "s1", None).add("a");
        assert!(!Seen::open(tmp.path(), "s2", None).contains("a"));
        assert!(!Seen::open(tmp.path(), "s1", Some("agent-7")).contains("a"));
    }

    #[test]
    fn an_id_cannot_leave_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        Seen::open(tmp.path(), "../../x", None).add("a");
        assert!(tmp.path().join("seen").read_dir().unwrap().count() == 1);
    }

    #[test]
    fn an_unwritable_place_does_not_fail() {
        let mut seen = Seen::open(std::path::Path::new("/proc/nope"), "s", None);
        seen.add("a");
        assert!(seen.contains("a"));
    }

    /// Pins the file content after two `add`s: exactly two clean lines, no
    /// stray blank line from a slug that was interleaved with another
    /// process's write. `strace` shows the WHY (one `write(2)` instead of
    /// two); this pins the WHAT.
    #[test]
    fn two_adds_produce_two_clean_lines_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let mut seen = Seen::open(tmp.path(), "s1", None);
        seen.add("a");
        seen.add("b");
        let path = tmp.path().join("seen").join("s1--main.txt");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw, "a\nb\n");
    }
}
