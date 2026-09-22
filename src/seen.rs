//! Which caveats a session has been shown already.
//!
//! The key is the session AND the agent: a subagent starts with a fresh
//! context and has not read what its parent was shown.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct Seen {
    path: Option<PathBuf>,
    slugs: BTreeSet<String>,
}

/// A `seen` file this long untouched belongs to a session or subagent
/// nobody is coming back to: two weeks.
const MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Drops every regular file in `seen_dir` whose `modified` time is older
/// than `MAX_AGE`. Errors — the directory does not exist yet, a file
/// vanished, its time is unreadable — are ignored: this must never fail
/// the hook it runs inside of.
fn prune_old(seen_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(seen_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        // `Err` here means `modified` is in the future relative to `now`:
        // not old, so left alone rather than treated as ancient.
        if now.duration_since(modified).is_ok_and(|age| age > MAX_AGE) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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
        let seen_dir = state_dir.join("seen");
        let path = seen_dir.join(name);
        // A new session or subagent: sweep `seen/` for files nobody will
        // return to before this one is created, then create this one's
        // file empty right away — so its mtime means "created or last
        // shown" and the sweep runs once per truly new key, not on every
        // hook call of a session that never shows a caveat. `add` bumps
        // the mtime again on every write; errors creating it here (the
        // directory can't be made, the disk is full) are ignored, same as
        // everywhere else in this module — `add` tries again on its own
        // write. A known session — the hot path, one hook call among many
        // in the same run — skips straight past with a single `stat`.
        if path.metadata().is_err() {
            prune_old(&seen_dir);
            let _ = std::fs::create_dir_all(&seen_dir);
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path);
        }
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

    /// Same claim as `another_session_and_a_subagent_start_fresh`, but with
    /// `session_id` and `agent_id` taken from a real recorded subagent
    /// PreToolUse event instead of a literal, and pinning that the two
    /// end up as two distinct files under `seen/`.
    #[test]
    fn a_subagent_has_its_own_seen_key() {
        const RAW: &str = include_str!("../tests/recorded/pre_tool_use_subagent.json");
        let event: serde_json::Value = serde_json::from_str(RAW).unwrap();
        let session = event["session_id"].as_str().unwrap();
        let agent = event["agent_id"].as_str().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let mut main = Seen::open(tmp.path(), session, None);
        main.add("a");
        assert!(main.contains("a"));

        let mut subagent = Seen::open(tmp.path(), session, Some(agent));
        assert!(!subagent.contains("a"));
        // `open` itself creates an empty file for a brand-new key (see
        // `open_creates_the_session_file_so_the_sweep_runs_once`), so both
        // files already exist at this point — write on the subagent's own
        // key too, so a caveat actually shown is part of this picture, not
        // just an empty file.
        subagent.add("b");

        let files: Vec<_> = tmp.path().join("seen").read_dir().unwrap().collect();
        assert_eq!(
            files.len(),
            2,
            "main and subagent must land in different files"
        );
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

    /// A `seen` file two weeks untouched has no session left to return to it:
    /// `open` for a brand-new session sweeps it away before creating its own
    /// file, but leaves a recent neighbour alone.
    #[test]
    fn open_removes_seen_files_older_than_two_weeks() {
        let tmp = tempfile::tempdir().unwrap();
        let seen_dir = tmp.path().join("seen");
        std::fs::create_dir_all(&seen_dir).unwrap();
        let old = seen_dir.join("old--main.txt");
        std::fs::write(&old, "a\n").unwrap();
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(15 * 86400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(old_time)
            .unwrap();
        let fresh = seen_dir.join("fresh--main.txt");
        std::fs::write(&fresh, "b\n").unwrap();

        Seen::open(tmp.path(), "brand-new-session", None);

        assert!(!old.exists(), "a file older than 14 days must be gone");
        assert!(fresh.exists(), "a recent neighbour must stay");
    }

    /// Opening a session whose file already exists is the hot path: no
    /// directory sweep, so an old neighbour survives untouched.
    #[test]
    fn open_of_a_known_session_does_not_touch_its_neighbours() {
        let tmp = tempfile::tempdir().unwrap();
        let seen_dir = tmp.path().join("seen");
        std::fs::create_dir_all(&seen_dir).unwrap();
        let old = seen_dir.join("old--main.txt");
        std::fs::write(&old, "a\n").unwrap();
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(15 * 86400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(old_time)
            .unwrap();
        std::fs::write(seen_dir.join("known--main.txt"), "b\n").unwrap();

        Seen::open(tmp.path(), "known", None);

        assert!(old.exists(), "a known session's open must not sweep");
    }

    /// `open` creates the session's file empty right away, not only on the
    /// first `add` — so its mtime means "created or last shown", and the
    /// 14-day sweep in `prune_old` runs once per truly NEW key, not on
    /// every hook call of a session that never shows a caveat.
    #[test]
    fn open_creates_the_session_file_so_the_sweep_runs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let seen_dir = tmp.path().join("seen");
        std::fs::create_dir_all(&seen_dir).unwrap();
        let neighbour = seen_dir.join("neighbour--main.txt");
        std::fs::write(&neighbour, "x\n").unwrap();

        // First open of a brand-new session: creates its own file (without
        // ever calling `add`) and, because the key was unknown, sweeps
        // `seen/` once — the neighbour is fresh, so it survives.
        Seen::open(tmp.path(), "brand-new", None);
        assert!(
            seen_dir.join("brand-new--main.txt").exists(),
            "open must create the session file itself, before any add"
        );
        assert!(neighbour.exists());

        // Age the neighbour past the sweep threshold, then open the SAME
        // session again. Its file now exists, so this is the hot path: no
        // sweep runs, and the now-old neighbour survives regardless.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(15 * 86400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&neighbour)
            .unwrap()
            .set_modified(old)
            .unwrap();
        Seen::open(tmp.path(), "brand-new", None);
        assert!(
            neighbour.exists(),
            "a known session's second open must not sweep"
        );
    }
}
