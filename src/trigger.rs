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
    pub fn matches(&self, command: &str) -> bool {
        match self {
            CommandMatcher::Anywhere(re) => re.is_match(command),
            CommandMatcher::Position { program, pattern } => {
                let Some(segments) = scan::commands(command) else {
                    return false;
                };
                segments.iter().any(|seg| {
                    scan::program(seg) == Some(*program)
                        && pattern.as_ref().is_none_or(|re| re.is_match(seg))
                })
            }
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
    /// `segments` (from `scan::commands`, computed once per hook call and
    /// shared across every trigger of every caveat). Most triggers do not
    /// apply to a given command at all, and the hook path must not pay a
    /// regex compile for each of them anyway. `where: anywhere` has no such
    /// gate — it always needs the regex, exactly as `matches` does.
    /// A pattern that fails to compile matches nothing here and never
    /// errors, same as `matches(..).unwrap_or(false)` does for the caller.
    pub fn matches_lazily(&self, command: &str, segments: Option<&[&str]>) -> bool {
        match self.place {
            Where::Anywhere => self.matches(command).unwrap_or(false),
            Where::Position => {
                let Some(program) = self.program.as_deref() else {
                    return false;
                };
                let Some(segments) = segments else {
                    return false;
                };
                segments.iter().any(|seg| {
                    scan::program(seg) == Some(program)
                        && match &self.pattern {
                            None => true,
                            Some(p) => Regex::new(p).is_ok_and(|re| re.is_match(seg)),
                        }
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
