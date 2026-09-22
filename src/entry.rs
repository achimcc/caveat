//! One caveat: a Markdown file with YAML frontmatter and the lesson below it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::index::{BEGIN, END};

/// Matches the text a tool call printed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputTrigger {
    /// A literal substring. The default: this project has paid enough for patterns.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    /// An example the trigger must match. Never executed.
    pub hits: String,
    /// An example the trigger must not match. Never executed.
    pub misses: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Where {
    /// Only where the shell would run a command.
    #[default]
    Position,
    /// Anywhere in the raw line, quoted or not: `ssh host '…'` hides its inner command.
    Anywhere,
}

/// Matches the command a tool call is about to run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandTrigger {
    #[serde(default, rename = "where")]
    pub place: Where,
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    pub hits: String,
    pub misses: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Front {
    title: String,
    line: String,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    output: Vec<OutputTrigger>,
    #[serde(default)]
    command: Vec<CommandTrigger>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// The file name without `.md`.
    pub slug: String,
    pub path: PathBuf,
    pub title: String,
    /// The half sentence for an index.
    pub line: String,
    pub since: Option<String>,
    pub family: Option<String>,
    pub output: Vec<OutputTrigger>,
    pub command: Vec<CommandTrigger>,
    pub body: String,
}

/// `field` (`"title"` or `"line"`) must be one line and free of the index
/// markers: a `\n`/`\r` breaks `index::render`'s one-line-per-entry shape,
/// and either marker text lets a caveat's own `title`/`line` masquerade as
/// `index::markers`' `BEGIN`/`END`, which would flip an unrelated
/// `check --index` to a doubled-marker tool error.
fn no_newline_or_marker(field: &str, value: &str) -> Result<()> {
    if value.contains('\n') || value.contains('\r') {
        bail!("`{field}` must not contain a newline");
    }
    if value.contains(BEGIN) || value.contains(END) {
        bail!("`{field}` must not contain the index marker");
    }
    Ok(())
}

pub fn parse(path: &Path, raw: &str) -> Result<Entry> {
    let slug = path
        .file_stem()
        .and_then(|s| s.to_str())
        .context("the file name is not UTF-8")?
        .to_string();
    let rest = raw
        .strip_prefix("---\n")
        .context("the file must start with a `---` line")?;
    let (front, body) = rest
        .split_once("\n---\n")
        .context("the frontmatter is not closed by a `---` line")?;
    let front: Front = serde_yaml_ng::from_str(front).context("frontmatter")?;
    if front.title.trim().is_empty() || front.line.trim().is_empty() {
        bail!("`title` and `line` must not be empty");
    }
    no_newline_or_marker("title", &front.title)?;
    no_newline_or_marker("line", &front.line)?;
    for t in &front.output {
        if t.text.is_some() == t.regex.is_some() {
            bail!("an output trigger needs exactly one of `text` and `regex`");
        }
    }
    for t in &front.command {
        match t.place {
            Where::Position if t.program.is_none() => {
                bail!("a command trigger needs `program`, or `where: anywhere` with `pattern`")
            }
            Where::Anywhere if t.pattern.is_none() || t.program.is_some() => {
                bail!("`where: anywhere` needs `pattern` and takes no `program`")
            }
            _ => {}
        }
    }
    Ok(Entry {
        slug,
        path: path.to_path_buf(),
        title: front.title,
        line: front.line,
        since: front.since,
        family: front.family,
        output: front.output,
        command: front.command,
        body: body.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const SAMPLE: &str = r#"---
title: CAP_SYS_PTRACE is systemd's own capability
line: PrivateUsers and podman need it; the message says ENOENT
since: 2026-09-20
family: message-names-the-wrong-thing
output:
  - text: "Failed to set up user namespacing"
    hits: "foo.service: Failed to set up user namespacing: No such file or directory"
    misses: "user namespacing is enabled"
command:
  - program: ssh
    pattern: '203\.0\.113\.7'
    hits: "ssh root@203.0.113.7 uptime"
    misses: "ssh -F ssh_config vps uptime"
  - where: anywhere
    pattern: 'journalctl\b[^|;&]*\s-k\b'
    hits: "ssh root@server 'journalctl -k -g x'"
    misses: "journalctl -g x"
---

Body **text**.
"#;

    #[test]
    fn a_file_becomes_an_entry() {
        let e = parse(Path::new("caveats/cap-sys-ptrace.md"), SAMPLE).unwrap();
        assert_eq!(e.slug, "cap-sys-ptrace");
        assert_eq!(e.title, "CAP_SYS_PTRACE is systemd's own capability");
        assert_eq!(e.since.as_deref(), Some("2026-09-20"));
        assert_eq!(e.output.len(), 1);
        assert_eq!(e.command[0].place, Where::Position);
        assert_eq!(e.command[1].place, Where::Anywhere);
        assert_eq!(e.body, "Body **text**.");
    }

    #[test]
    fn an_entry_without_triggers_is_allowed() {
        let e = parse(Path::new("x.md"), "---\ntitle: T\nline: L\n---\nB\n").unwrap();
        assert!(e.output.is_empty() && e.command.is_empty());
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let raw = "---\ntitle: T\nline: L\ntitel: oops\n---\nB\n";
        assert!(parse(Path::new("x.md"), raw).is_err());
    }

    #[test]
    fn an_output_trigger_needs_exactly_one_of_text_and_regex() {
        let both = "---\ntitle: T\nline: L\noutput:\n  - text: a\n    regex: a\n    hits: a\n    misses: b\n---\nB\n";
        let none = "---\ntitle: T\nline: L\noutput:\n  - hits: a\n    misses: b\n---\nB\n";
        assert!(parse(Path::new("x.md"), both).is_err());
        assert!(parse(Path::new("x.md"), none).is_err());
    }

    #[test]
    fn a_command_trigger_needs_a_program_or_anywhere_with_a_pattern() {
        let bare = "---\ntitle: T\nline: L\ncommand:\n  - pattern: a\n    hits: a\n    misses: b\n---\nB\n";
        let anywhere = "---\ntitle: T\nline: L\ncommand:\n  - where: anywhere\n    hits: a\n    misses: b\n---\nB\n";
        assert!(parse(Path::new("x.md"), bare).is_err());
        assert!(parse(Path::new("x.md"), anywhere).is_err());
    }

    #[test]
    fn a_file_without_frontmatter_is_an_error() {
        assert!(parse(Path::new("x.md"), "just text\n").is_err());
    }

    /// A `line` carrying an index end marker would otherwise flip the next
    /// `check --index` to exit 2 (`markers` sees two `END`s); a `title` or
    /// `line` with an embedded newline breaks the one-line index shape
    /// (`index::render` puts each entry on exactly one line). Both are
    /// caught at parse time, before either module ever sees the value.
    #[test]
    fn a_title_or_line_with_a_newline_is_an_error() {
        let bad_title = "---\ntitle: \"a\\nb\"\nline: L\n---\nB\n";
        let bad_line = "---\ntitle: T\nline: \"a\\nb\"\n---\nB\n";
        assert!(parse(Path::new("x.md"), bad_title).is_err());
        assert!(parse(Path::new("x.md"), bad_line).is_err());
    }

    #[test]
    fn a_line_containing_the_index_marker_is_an_error() {
        let begin = "---\ntitle: T\nline: \"has <!-- caveat:index --> in it\"\n---\nB\n";
        let end = "---\ntitle: T\nline: \"has <!-- /caveat:index --> in it\"\n---\nB\n";
        assert!(parse(Path::new("x.md"), begin).is_err());
        assert!(parse(Path::new("x.md"), end).is_err());
    }
}
