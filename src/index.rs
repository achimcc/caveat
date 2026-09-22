//! An index of every caveat, spliced into a Markdown file between two
//! marker comments (`CLAUDE.md`, typically) — `caveat gen --index FILE`
//! writes it, `caveat check --index FILE` says when it has gone stale.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::entry::Entry;

pub const BEGIN: &str = "<!-- caveat:index -->";
pub const END: &str = "<!-- /caveat:index -->";

/// `file`'s path relative to `dir`, when `file` lies under `dir`; `file`
/// itself otherwise. Pure — no filesystem access — so it only means what
/// its name says when both arguments are already canonical; `render` is
/// the caller that makes them so.
fn relative(dir: &Path, file: &Path) -> PathBuf {
    file.strip_prefix(dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| file.to_path_buf())
}

/// The path an index line shows for `entry_path`, computed by
/// canonicalizing both `index_file`'s parent directory and `entry_path` and
/// then taking the first relative to the second — the `pathdiff`-free
/// recipe from the plan. Either side failing to canonicalize (a path that
/// does not exist, an index file with no parent) falls back to
/// `entry_path` unchanged, same as a directory that turns out not to be an
/// ancestor.
fn caveat_path_for_index(index_file: &Path, entry_path: &Path) -> PathBuf {
    let index_dir = index_file.parent().unwrap_or_else(|| Path::new("."));
    match (
        std::fs::canonicalize(index_dir),
        std::fs::canonicalize(entry_path),
    ) {
        (Ok(dir), Ok(file)) => relative(&dir, &file),
        _ => entry_path.to_path_buf(),
    }
}

/// One line per entry, sorted by `since` ascending (entries without `since`
/// last), then by slug. `rel` is the caveat's path relative to the index
/// file's directory. Ends with a newline; empty when there are no entries.
pub fn render(entries: &[Entry], index_file: &Path) -> String {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| {
        (a.since.is_none(), a.since.as_deref(), a.slug.as_str()).cmp(&(
            b.since.is_none(),
            b.since.as_deref(),
            b.slug.as_str(),
        ))
    });
    let mut out = String::new();
    for e in sorted {
        let rel = caveat_path_for_index(index_file, &e.path);
        out.push_str(&format!(
            "- **{}** — {} (`{}`)\n",
            e.title,
            e.line,
            rel.display()
        ));
    }
    out
}

/// The byte offset of the one and only `BEGIN` and the one and only `END`,
/// with `END` at or after `BEGIN`. Anything else — a missing marker, a
/// doubled one, `END` before `BEGIN`, a marker sharing its line with other
/// text — is a tool error, not a finding: the file is not in the shape
/// this module can maintain. A marker only counts on a line that, after
/// trimming one trailing `\r` (a CRLF file), equals the marker exactly —
/// `splice`'s `text[..begin + BEGIN.len()]` and `text[end..]` slicing
/// depends on that: a marker with leading or trailing text on its line
/// would otherwise cut mid-line.
fn markers(text: &str) -> Result<(usize, usize)> {
    let mut begin = None;
    let mut end = None;
    let mut offset = 0usize;
    for raw_line in text.split_inclusive('\n') {
        let without_newline = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        let has_begin = line.contains(BEGIN);
        let has_end = line.contains(END);
        if has_begin || has_end {
            if line != BEGIN && line != END {
                bail!("a marker must stand alone on its line: {line:?}");
            }
            if has_begin {
                if begin.is_some() {
                    bail!("{BEGIN} marker appears more than once");
                }
                begin = Some(offset);
            }
            if has_end {
                if end.is_some() {
                    bail!("{END} marker appears more than once");
                }
                end = Some(offset);
            }
        }
        offset += raw_line.len();
    }
    let begin = begin.ok_or_else(|| anyhow::anyhow!("{BEGIN} marker is missing"))?;
    let end = end.ok_or_else(|| anyhow::anyhow!("{END} marker is missing"))?;
    if end < begin {
        bail!("{END} marker precedes {BEGIN}");
    }
    Ok((begin, end))
}

/// The file's text with everything between `BEGIN` and `END` replaced by
/// `block`. Errors when a marker is missing or appears more than once, or
/// `END` precedes `BEGIN`.
pub fn splice(text: &str, block: &str) -> Result<String> {
    let (begin, end) = markers(text)?;
    let before = &text[..begin + BEGIN.len()];
    let after = &text[end..];
    Ok(format!("{before}\n{block}{after}"))
}

/// What currently stands between the markers (same errors as `splice`).
///
/// Compared byte-for-byte against `render`'s output, so a hand-written
/// index block using CRLF line endings reads as out of date even when its
/// content otherwise matches: `check_index` reports one finding for it,
/// `gen` then normalises the block to LF when it writes, and a second
/// `check_index` afterwards is clean (measured: 1 finding, then 0 after
/// `gen`).
pub fn current_block(text: &str) -> Result<String> {
    let (begin, end) = markers(text)?;
    let inner = &text[begin + BEGIN.len()..end];
    Ok(inner.strip_prefix('\n').unwrap_or(inner).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(slug: &str, since: Option<&str>) -> Entry {
        Entry {
            slug: slug.to_string(),
            path: PathBuf::from(format!("caveats/{slug}.md")),
            title: format!("Title {slug}"),
            line: format!("Line {slug}"),
            since: since.map(String::from),
            family: None,
            output: Vec::new(),
            command: Vec::new(),
            body: String::new(),
        }
    }

    #[test]
    fn render_sorts_by_since_then_slug() {
        let entries = [
            entry("mid", Some("2026-09-10")),
            entry("last", None),
            entry("first", Some("2026-09-01")),
        ];
        let rendered = render(&entries, Path::new("/x/CLAUDE.md"));
        let order: Vec<&str> = rendered
            .lines()
            .map(|l| l.split("**").nth(1).expect("each line has a `**title**`"))
            .collect();
        assert_eq!(order, ["Title first", "Title mid", "Title last"]);
    }

    #[test]
    fn render_uses_a_path_relative_to_the_index_file() {
        assert_eq!(
            relative(Path::new("/x"), Path::new("/x/caveats/a.md")),
            PathBuf::from("caveats/a.md")
        );
    }

    #[test]
    fn splice_replaces_only_between_the_markers() {
        let text = format!("before\n{BEGIN}\nalt\n{END}\nafter\n");
        let spliced = splice(&text, "- **A** — a (`x`)\n").unwrap();
        assert!(spliced.starts_with(&format!("before\n{BEGIN}\n")));
        assert!(spliced.ends_with(&format!("{END}\nafter\n")));
        assert!(spliced.contains("- **A** — a (`x`)\n"));
        assert!(!spliced.contains("alt"));
    }

    #[test]
    fn splice_rejects_missing_or_duplicate_markers() {
        assert!(splice(&format!("no begin\n{END}\n"), "x").is_err());
        assert!(splice(&format!("{BEGIN}\nno end\n"), "x").is_err());
        assert!(splice(&format!("{BEGIN}\n{BEGIN}\n{END}\n"), "x").is_err());
    }

    /// README's command reference for `gen --index`/`check --index` already
    /// states the precondition ("each expected on its own line exactly
    /// once") — this pins that `markers` enforces it: a marker sharing its
    /// line with other text (before, after, or CRLF's `\r` aside) is a
    /// marker error, same as a missing or doubled one.
    #[test]
    fn a_marker_that_is_not_alone_on_its_line_is_an_error() {
        assert!(splice(&format!("prefix {BEGIN}\n{END}\n"), "x").is_err());
        assert!(splice(&format!("{BEGIN}\n{END} suffix\n"), "x").is_err());
        assert!(splice(&format!("{BEGIN} and {END} on one line\n"), "x").is_err());
        // A trailing `\r` alone (CRLF) is still "alone on its line".
        assert!(splice(&format!("{BEGIN}\r\nalt\r\n{END}\r\n"), "x").is_ok());
    }

    #[test]
    fn current_block_is_what_splice_wrote() {
        let text = format!("before\n{BEGIN}\nalt\n{END}\nafter\n");
        let block = "- **A** — a (`x`)\n- **B** — b (`y`)\n";
        let spliced = splice(&text, block).unwrap();
        assert_eq!(current_block(&spliced).unwrap(), block);
    }
}
