//! Does a trigger match?

use anyhow::{Context, Result, bail};
use regex::Regex;

use crate::entry::{CommandTrigger, OutputTrigger, Where};
use crate::scan;

impl OutputTrigger {
    pub fn matches(&self, haystack: &str) -> Result<bool> {
        match (&self.text, &self.regex) {
            (Some(text), None) => Ok(haystack.contains(text.as_str())),
            (None, Some(re)) => Ok(Regex::new(re)
                .with_context(|| format!("regex `{re}`"))?
                .is_match(haystack)),
            _ => bail!("an output trigger needs exactly one of `text` and `regex`"),
        }
    }
}

impl CommandTrigger {
    pub fn matches(&self, command: &str) -> Result<bool> {
        let re = self
            .pattern
            .as_deref()
            .map(|p| Regex::new(p).with_context(|| format!("pattern `{p}`")))
            .transpose()?;
        match self.place {
            Where::Anywhere => {
                let re = re.context("`where: anywhere` needs `pattern`")?;
                Ok(re.is_match(command))
            }
            Where::Position => {
                let program = self
                    .program
                    .as_deref()
                    .context("a command trigger needs `program`")?;
                let Some(segments) = scan::commands(command) else {
                    return Ok(false);
                };
                Ok(segments.iter().any(|seg| {
                    scan::program(seg) == Some(program)
                        && re.as_ref().is_none_or(|re| re.is_match(seg))
                }))
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
        assert!(
            !t.matches("git commit -m \"ssh root@203.0.113.7\"")
                .unwrap()
        );
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
}
