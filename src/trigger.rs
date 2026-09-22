//! Does a trigger match?
//!
//! `OutputTrigger`/`CommandTrigger` carry the trigger as written (a `String`
//! pattern, still to be compiled). `compile()` turns one into an
//! `OutputMatcher`/`CommandMatcher` that holds an already-built `Regex` (or,
//! for a literal `text`, just borrows the string — no allocation happens
//! there at all). Compilation is where a broken pattern becomes an error;
//! the matcher itself never fails. Every hot path — `check`, `hook`,
//! `search` — compiles a trigger once and then calls `matches` as many
//! times as it needs, instead of recompiling its regex on every call the
//! way the pre-0.2 `matches` on the trigger types did.
//!
//! `entry::parse` is what enforces "exactly one of `text`/`regex`" on an
//! output trigger and "`program` xor `where: anywhere` + `pattern`" on a
//! command trigger — every `Entry` reaching this module already satisfies
//! that. The checks `compile()` repeats here only guard against a
//! `OutputTrigger`/`CommandTrigger` built by hand (as the unit tests below
//! do), bypassing `parse`.

use anyhow::{Context, Result, bail};
use regex::Regex;

use crate::entry::{CommandTrigger, OutputTrigger, Where};
use crate::scan;

/// A compiled `OutputTrigger`. Borrows the trigger it came from.
pub enum OutputMatcher<'a> {
    Text(&'a str),
    Regex(Regex),
}

impl OutputMatcher<'_> {
    pub fn matches(&self, haystack: &str) -> bool {
        match self {
            OutputMatcher::Text(text) => haystack.contains(text),
            OutputMatcher::Regex(re) => re.is_match(haystack),
        }
    }
}

/// A compiled `CommandTrigger`. Borrows the trigger it came from.
pub enum CommandMatcher<'a> {
    Anywhere(Regex),
    Position {
        program: &'a str,
        pattern: Option<Regex>,
    },
}

impl CommandMatcher<'_> {
    /// The whole `where: position`/`where: anywhere` decision — the one
    /// place it lives. Takes already-scanned segments instead of scanning
    /// `command` itself, so a caller that scanned once for many triggers
    /// (the hook path, through `Scanned`) does not scan again per trigger.
    /// `segments`, when given, MUST be `scan::commands(command)` for this
    /// exact `command`; nothing here can check that agreement, which is
    /// why `matches_lazily` only ever reaches this through `Scanned`.
    pub fn matches_with(&self, command: &str, segments: Option<&[&str]>) -> bool {
        match self {
            CommandMatcher::Anywhere(re) => re.is_match(command),
            CommandMatcher::Position { program, pattern } => {
                let Some(segments) = segments else {
                    return false;
                };
                segments.iter().any(|seg| {
                    scan::program(seg) == Some(*program)
                        && pattern.as_ref().is_none_or(|re| re.is_match(seg))
                })
            }
        }
    }

    pub fn matches(&self, command: &str) -> bool {
        // `Anywhere` never looks at segments; do not pay for the scan.
        let segments = match self {
            CommandMatcher::Anywhere(_) => None,
            CommandMatcher::Position { .. } => scan::commands(command),
        };
        self.matches_with(command, segments.as_deref())
    }
}

/// A command together with its own scan (`scan::commands`), computed once.
/// The hook path shares one `Scanned` across every trigger of every
/// caveat for a given call instead of rescanning per trigger — and,
/// because `Scanned::of` is the only way to build one, `matches_lazily`
/// can never be handed segments that belong to a different command than
/// the one it also matches against.
pub struct Scanned<'a> {
    command: &'a str,
    segments: Option<Vec<&'a str>>,
}

impl<'a> Scanned<'a> {
    pub fn of(command: &'a str) -> Self {
        Scanned {
            command,
            segments: scan::commands(command),
        }
    }
}

/// Implemented by both compiled matcher kinds, so callers that only need
/// "does it match" (`check::examine`) can stay generic over which kind of
/// trigger they were handed.
pub(crate) trait Matches {
    fn matches(&self, haystack: &str) -> bool;
}

impl Matches for OutputMatcher<'_> {
    fn matches(&self, haystack: &str) -> bool {
        OutputMatcher::matches(self, haystack)
    }
}

impl Matches for CommandMatcher<'_> {
    fn matches(&self, haystack: &str) -> bool {
        CommandMatcher::matches(self, haystack)
    }
}

impl OutputTrigger {
    pub fn compile(&self) -> Result<OutputMatcher<'_>> {
        match (&self.text, &self.regex) {
            (Some(text), None) => Ok(OutputMatcher::Text(text.as_str())),
            (None, Some(re)) => Ok(OutputMatcher::Regex(
                Regex::new(re).with_context(|| format!("regex `{re}`"))?,
            )),
            _ => bail!("an output trigger needs exactly one of `text` and `regex`"),
        }
    }

    pub fn matches(&self, haystack: &str) -> Result<bool> {
        Ok(self.compile()?.matches(haystack))
    }
}

impl CommandTrigger {
    pub fn compile(&self) -> Result<CommandMatcher<'_>> {
        let re = self
            .pattern
            .as_deref()
            .map(|p| Regex::new(p).with_context(|| format!("pattern `{p}`")))
            .transpose()?;
        match self.place {
            Where::Anywhere => {
                let re = re.context("`where: anywhere` needs `pattern`")?;
                Ok(CommandMatcher::Anywhere(re))
            }
            Where::Position => {
                let program = self
                    .program
                    .as_deref()
                    .context("a command trigger needs `program`")?;
                Ok(CommandMatcher::Position {
                    program,
                    pattern: re,
                })
            }
        }
    }

    pub fn matches(&self, command: &str) -> Result<bool> {
        Ok(self.compile()?.matches(command))
    }

    /// Like `matches`, but for `where: position` never touches `pattern`
    /// unless `program` is already found at command position in
    /// `scanned` (shared across every trigger of every caveat for one
    /// hook call, instead of rescanned per trigger). Most triggers do not
    /// apply to a given command at all, and the hook path must not pay a
    /// regex compile for each of them anyway. Once `program` is found, the
    /// actual decision — does some segment satisfy both `program` and
    /// `pattern` — is `CommandMatcher::matches_with`'s alone; this only
    /// gates whether that compile happens at all, and it compiles at most
    /// once per call, not once per matching segment. `where: anywhere` has
    /// no such gate — it always needs the regex, exactly as `matches` does.
    /// A pattern that fails to compile matches nothing here and never
    /// errors, same as `matches(..).unwrap_or(false)` does for the caller.
    pub fn matches_lazily(&self, scanned: &Scanned<'_>) -> bool {
        match self.place {
            Where::Anywhere => self.matches(scanned.command).unwrap_or(false),
            Where::Position => {
                let Some(program) = self.program.as_deref() else {
                    return false;
                };
                let Some(segments) = scanned.segments.as_deref() else {
                    return false;
                };
                let program_occurs = segments
                    .iter()
                    .any(|seg| scan::program(seg) == Some(program));
                program_occurs
                    && self
                        .compile()
                        .is_ok_and(|m| m.matches_with(scanned.command, Some(segments)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scanned;
    use crate::entry::{CommandTrigger, OutputTrigger, Where};

    fn out(text: Option<&str>, regex: Option<&str>) -> OutputTrigger {
        OutputTrigger {
            text: text.map(String::from),
            regex: regex.map(String::from),
            hits: String::new(),
            misses: String::new(),
        }
    }

    fn cmd(place: Where, program: Option<&str>, pattern: Option<&str>) -> CommandTrigger {
        CommandTrigger {
            place,
            program: program.map(String::from),
            pattern: pattern.map(String::from),
            hits: String::new(),
            misses: String::new(),
        }
    }

    #[test]
    fn text_is_a_literal_case_sensitive_substring() {
        let t = out(Some("a.b (c)"), None);
        assert!(t.matches("xx a.b (c) yy").unwrap());
        assert!(!t.matches("xx aXb (c) yy").unwrap());
        assert!(!t.matches("xx A.B (C) yy").unwrap());
    }

    #[test]
    fn regex_is_a_regex_and_a_broken_one_is_an_error() {
        assert!(
            out(None, Some(r"set in sops\.secrets\.\w+"))
                .matches("set in sops.secrets.foo")
                .unwrap()
        );
        assert!(out(None, Some("(")).matches("x").is_err());
    }

    #[test]
    fn a_program_matches_only_at_command_position() {
        let t = cmd(Where::Position, Some("ssh"), Some(r"203\.0\.113\.7"));
        assert!(t.matches("cd /x && ssh root@203.0.113.7 uptime").unwrap());
        assert!(
            t.matches("sudo timeout 60 /usr/bin/ssh root@203.0.113.7")
                .unwrap()
        );
        assert!(!t.matches("ssh -F ssh_config vps uptime").unwrap());
        assert!(!t.matches("echo 'ssh root@203.0.113.7'").unwrap());
        assert!(!t.matches("git commit -m \"ssh root@203.0.113.7\"").unwrap());
    }

    #[test]
    fn a_program_without_a_pattern_matches_every_call_of_it() {
        assert!(
            cmd(Where::Position, Some("pkill"), None)
                .matches("pkill -f nix")
                .unwrap()
        );
    }

    #[test]
    fn a_pattern_does_not_reach_into_the_next_command() {
        let t = cmd(Where::Position, Some("ssh"), Some("uptime"));
        assert!(!t.matches("ssh host true; uptime").unwrap());
    }

    #[test]
    fn anywhere_sees_the_inner_command_of_an_ssh_call() {
        let t = cmd(Where::Anywhere, None, Some(r"journalctl\b[^|;&]*\s-k\b"));
        assert!(t.matches("ssh root@server 'journalctl -k -g x'").unwrap());
        assert!(!t.matches("ssh root@server 'journalctl -g x'").unwrap());
    }

    #[test]
    fn a_line_the_scanner_does_not_follow_matches_nothing_at_position() {
        let t = cmd(Where::Position, Some("ssh"), None);
        assert!(!t.matches("cat <<EOF\nssh host\nEOF").unwrap());
    }

    #[test]
    fn an_array_literal_naming_the_program_is_not_a_command() {
        let t = cmd(Where::Position, Some("ssh"), None);
        assert!(!t.matches("tools=(ssh scp rsync)").unwrap());
    }

    #[test]
    fn compile_fails_for_a_broken_regex() {
        assert!(out(None, Some("(")).compile().is_err());
        assert!(cmd(Where::Anywhere, None, Some("(")).compile().is_err());
    }

    #[test]
    fn a_compiled_text_matcher_is_case_sensitive_literal() {
        let t = out(Some("a.b (c)"), None);
        let m = t.compile().unwrap();
        assert!(m.matches("xx a.b (c) yy"));
        assert!(!m.matches("xx aXb (c) yy"));
        assert!(!m.matches("xx A.B (C) yy"));
    }

    #[test]
    fn a_compiled_position_matcher_behaves_like_today() {
        let t = cmd(Where::Position, Some("ssh"), Some(r"203\.0\.113\.7"));
        let m = t.compile().unwrap();
        assert!(m.matches("cd /x && ssh root@203.0.113.7 uptime"));
        assert!(m.matches("sudo timeout 60 /usr/bin/ssh root@203.0.113.7"));
        assert!(!m.matches("ssh -F ssh_config vps uptime"));
        assert!(!m.matches("echo 'ssh root@203.0.113.7'"));
        assert!(!m.matches("git commit -m \"ssh root@203.0.113.7\""));
    }

    /// `matches_lazily` (the gated path the hook uses) must never disagree
    /// with `matches` (the always-compile path `check` and `search` use) —
    /// over a table of triggers and a table of commands designed to hit
    /// every branch: a plain program match, a segment the scanner does not
    /// follow, an array literal that merely names the program, a pattern
    /// that must not reach into the next command, a pipe, and a leading
    /// assignment.
    #[test]
    fn matches_lazily_agrees_with_matches_on_a_table_of_commands() {
        let triggers: Vec<(&str, CommandTrigger)> = vec![
            (
                "position with pattern",
                cmd(Where::Position, Some("ssh"), Some(r"203\.0\.113\.7")),
            ),
            (
                "position without pattern",
                cmd(Where::Position, Some("ssh"), None),
            ),
            (
                "position with a broken pattern",
                cmd(Where::Position, Some("ssh"), Some("(")),
            ),
            ("anywhere", cmd(Where::Anywhere, None, Some(r"ssh\b"))),
        ];
        let commands = [
            "ssh root@203.0.113.7 uptime",
            "ssh a; ssh root@203.0.113.7",
            "cat <<EOF\nssh host\nEOF",
            "echo 'ssh root@203.0.113.7'",
            "tools=(ssh scp)",
            "ssh host true; uptime",
            "ls | ssh host",
            "FOO=1 ssh host",
        ];
        for (label, t) in &triggers {
            for command in commands {
                let lazy = t.matches_lazily(&Scanned::of(command));
                let full = t.matches(command).unwrap_or(false);
                assert_eq!(
                    lazy, full,
                    "trigger `{label}`, command {command:?}: matches_lazily={lazy}, matches={full}"
                );
            }
        }
    }
}
