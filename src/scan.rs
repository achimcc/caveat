//! Where, in a shell command line, a command starts.
//!
//! Taken from lotse's hook scanner and as conservative: whatever this scanner
//! does not understand yields `None`, and the caller then matches nothing.

const KEYWORDS: &[&str] = &[
    "then", "do", "else", "elif", "if", "while", "until", "!", "time", "{",
];

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    is_identifier(name)
}

/// `NAME=(…` or `NAME+=(…`: an array literal or an append to one, not a
/// subshell around a command. `is_assignment` alone does not catch this —
/// it only looks at what comes before the first `=`. And unlike a plain
/// word, this check stays needed even with the bare-`(` fix in `commands()`
/// below: `is_assignment` recognises `NAME=(…` as an assignment and, by
/// design (same as `FOO=1 ssh host`), does NOT clear `command_position` —
/// so when the `(` right after it is reached, `command_position` is still
/// `true` and the bare-`(` fix would let it through as a real subshell.
/// (`declare -a x=(…` is different: there the array literal is not the
/// FIRST word, so the array word is never examined for a push at all, and
/// it is the bare-`(` fix that catches it, at the point of the `(` itself.)
fn starts_array_assignment(word: &str) -> bool {
    ["+=(", "=("].iter().any(|sep| {
        word.split_once(sep)
            .is_some_and(|(name, _)| is_identifier(name))
    })
}

/// The simple command that starts at `from`, cut at the first character that
/// could end it. Cutting inside a quoted string only shortens what a pattern
/// sees; it never lets it read into the NEXT command.
fn segment(text: &str, from: usize) -> &str {
    let rest = &text[from..];
    let end = rest.find([';', '|', '&', ')', '\n']).unwrap_or(rest.len());
    &rest[..end]
}

/// The simple commands of a line, each from its command position.
pub fn commands(text: &str) -> Option<Vec<&str>> {
    #[derive(PartialEq)]
    enum Ctx {
        Paren,
        DQuote,
    }
    let bytes = text.as_bytes();
    let mut stack: Vec<Ctx> = Vec::new();
    let mut out = Vec::new();
    let mut command_position = true;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if stack.last() == Some(&Ctx::DQuote) {
            match c {
                b'\\' => i += 1,
                b'"' => {
                    stack.pop();
                }
                b'`' => return None,
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    if bytes.get(i + 2) == Some(&b'(') {
                        return None;
                    }
                    stack.push(Ctx::Paren);
                    command_position = true;
                    i += 1;
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        match c {
            b' ' | b'\t' => i += 1,
            b'\n' | b';' => {
                command_position = true;
                i += 1;
            }
            b'|' | b'&' => {
                // `2>&1` and `>&2` are redirections, not the end of a command.
                let redirection = c == b'&' && i > 0 && bytes[i - 1] == b'>';
                if !redirection {
                    command_position = true;
                }
                i += 1;
            }
            b'(' => {
                // A bare `(` away from a command position is never a
                // subshell in shell grammar: it is an array literal
                // (`declare -a x=(…`), a function definition (`f() {`),
                // a `[[ … =~ (…) ]]` group, a case pattern, an extglob, or
                // `for ((…))`. Only a `(` AT a command position opens a
                // subshell — command substitution (`$(`) is a separate
                // branch and stays followed either way.
                if !command_position {
                    return None;
                }
                stack.push(Ctx::Paren);
                command_position = true;
                i += 1;
            }
            b')' => {
                if stack.pop() != Some(Ctx::Paren) {
                    return None;
                }
                command_position = false;
                i += 1;
            }
            b'#' if command_position || i == 0 || bytes[i - 1].is_ascii_whitespace() => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'`' => return None,
            b'<' if bytes.get(i + 1) == Some(&b'<') => return None,
            _ => {
                let start = i;
                if command_position {
                    let seg = segment(text, start);
                    let word = seg.split_whitespace().next().unwrap_or("");
                    // `[[ … ]]` and `case … esac` are not command lists;
                    // skipping just this word is not enough. `[[` still
                    // needs its own check even with the bare-`(` fix below:
                    // `&&`/`|` inside `[[ … ]]` reopens a command position
                    // on its own, with no `(` involved at all (confirmed:
                    // `[[ -n $a && ssh == $b ]]` wrongly matches `ssh`
                    // without this check). `case` no longer strictly needs
                    // one for the shapes tried (a pattern's `(…)` is now
                    // caught by the bare-`(` fix, and a bare pattern's
                    // closing `)` hits the paren-imbalance check below with
                    // nothing on the stack to pop) — kept anyway: a case
                    // statement is still not a command list, and removing
                    // the check was not proven safe for every pattern shape
                    // (alternation, extglob, quoting), only the ones tested.
                    if word == "[[" || word == "case" || starts_array_assignment(word) {
                        return None;
                    }
                    if !(KEYWORDS.contains(&word) || is_assignment(word)) {
                        out.push(seg);
                        command_position = false;
                    }
                }
                while i < bytes.len() {
                    match bytes[i] {
                        b' ' | b'\t' | b'\n' | b';' | b'|' | b'&' | b'(' | b')' => break,
                        b'\\' => i += 2,
                        b'\'' => {
                            i += 1;
                            while i < bytes.len() && bytes[i] != b'\'' {
                                i += 1;
                            }
                            if i >= bytes.len() {
                                return None;
                            }
                            i += 1;
                        }
                        b'"' => {
                            stack.push(Ctx::DQuote);
                            i += 1;
                            break;
                        }
                        b'`' => return None,
                        // ANSI-C quoting: this scanner's single-quote skip
                        // above is naive and closes early on an escaped `\'`
                        // inside `$'…'` — understood no further.
                        b'$' if bytes.get(i + 1) == Some(&b'\'') => return None,
                        b'$' if bytes.get(i + 1) == Some(&b'(') => {
                            if bytes.get(i + 2) == Some(&b'(') {
                                return None;
                            }
                            stack.push(Ctx::Paren);
                            command_position = true;
                            i += 2;
                            break;
                        }
                        b'<' if bytes.get(i + 1) == Some(&b'<') => return None,
                        _ => i += 1,
                    }
                }
                // A trailing backslash walks past the end: a line this scanner does not
                // understand, like lotse's.
                if i > bytes.len() {
                    return None;
                }
            }
        }
    }
    stack.is_empty().then_some(out)
}

/// The program a simple command runs: without its path, behind the wrappers
/// that only change HOW it runs.
pub fn program(segment: &str) -> Option<&str> {
    let mut words = segment.split_whitespace().peekable();
    loop {
        let word = words.next()?;
        match word {
            "sudo" | "nice" | "exec" | "command" => {}
            "timeout" => {
                // Its options, then the duration.
                while let Some(option) = words.next_if(|w| w.starts_with('-')) {
                    // `-k 5` and `-s KILL` carry a value of their own;
                    // `--signal=KILL` carries it in the same word.
                    if matches!(option, "-k" | "-s") {
                        words.next();
                    }
                }
                words.next()?;
            }
            "lotse" => {
                // `lotse run --class=… -- <command>`
                for w in words.by_ref() {
                    if w == "--" {
                        break;
                    }
                }
            }
            _ => return Some(word.rsplit('/').next().unwrap_or(word)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(text: &str) -> Vec<String> {
        commands(text)
            .unwrap()
            .into_iter()
            .map(|s| s.trim().to_string())
            .collect()
    }

    #[test]
    fn a_compound_line_is_split_at_command_positions() {
        assert_eq!(
            cmds("cd /x && ssh root@1.2.3.4 uptime; ls | wc -l"),
            ["cd /x", "ssh root@1.2.3.4 uptime", "ls", "wc -l"]
        );
    }

    #[test]
    fn quoted_text_is_not_a_command() {
        assert_eq!(
            cmds("echo \"ssh root@1.2.3.4\""),
            ["echo \"ssh root@1.2.3.4\""]
        );
        assert_eq!(cmds("git commit -m 'ssh root@1.2.3.4'").len(), 1);
    }

    #[test]
    fn assignments_and_keywords_come_before_the_command() {
        assert_eq!(cmds("FOO=1 ssh host"), ["ssh host"]);
        assert_eq!(cmds("if true; then ssh host; fi")[1], "ssh host");
    }

    #[test]
    fn a_command_substitution_is_followed() {
        assert!(cmds("x=$(ssh host uptime)").contains(&"ssh host uptime".to_string()));
    }

    #[test]
    fn a_redirection_does_not_start_a_command() {
        assert_eq!(cmds("make 2>&1").len(), 1);
    }

    #[test]
    fn what_the_scanner_does_not_follow_yields_none() {
        assert!(commands("cat <<EOF\nssh host\nEOF").is_none());
        assert!(commands("echo `ssh host`").is_none());
        assert!(commands("echo $((1+1))").is_none());
        assert!(commands("echo 'open").is_none());
        assert!(commands("\\").is_none());
        assert!(commands("echo \\").is_none());
        // An array literal or append: `(` after `NAME=`/`NAME+=` is not a
        // subshell around a command, it is a list of words.
        assert!(commands("tools=(ssh scp rsync)").is_none());
        assert!(commands("tools+=(ssh scp)").is_none());
        // `[[ … ]]` is a conditional expression, not a command list — `&&`
        // inside it does not open a command position.
        assert!(commands("[[ $cmd =~ ^(ssh|scp)$ ]] && echo yes").is_none());
        assert!(commands("[[ -n $a && ssh == $b ]]").is_none());
        // `case … in (pattern) …` — the `(` around a pattern is not a
        // subshell either.
        assert!(commands("case $x in (ssh) echo a;; esac").is_none());
        // ANSI-C quoting: an escaped `'` inside `$'…'` does not close it.
        assert!(commands("echo $'it\\'s' ; ssh host").is_none());
        // A bare `(` away from a command position is never a subshell: an
        // array literal on `declare`/`local`/`export` (the array itself is
        // not the first word here, unlike `tools=(ssh scp)` above), a
        // function definition, or `for ((…))`.
        assert!(commands("declare -a tools=(ssh scp)").is_none());
        assert!(commands("local tools=(ssh scp)").is_none());
        assert!(commands("export TOOLS=(ssh scp)").is_none());
        assert!(commands("f() { ssh host; }").is_none());
        assert!(commands("for ((i=0; i<3; i++)); do ssh host; done").is_none());
    }

    #[test]
    fn a_subshell_at_command_position_is_followed() {
        assert!(cmds("(cd /x && ssh host)").contains(&"ssh host".to_string()));
        assert!(cmds("ls | (ssh host)").contains(&"ssh host".to_string()));
        assert!(cmds("x=$(ssh host uptime)").contains(&"ssh host uptime".to_string()));
        assert!(cmds("echo \"$(ssh host)\"").contains(&"ssh host".to_string()));
    }

    #[test]
    fn text_beyond_ascii_is_walked_without_harm() {
        assert_eq!(cmds("echo „größer“ && ls").len(), 2);
    }

    #[test]
    fn the_program_is_found_behind_wrappers_and_paths() {
        assert_eq!(program("ssh -F x vps"), Some("ssh"));
        assert_eq!(program("sudo timeout -k 5 60 ssh vps"), Some("ssh"));
        assert_eq!(
            program("lotse run --class=eval -- nix build .#x"),
            Some("nix")
        );
        assert_eq!(
            program("/run/current-system/sw/bin/journalctl -k"),
            Some("journalctl")
        );
        assert_eq!(program("   "), None);
    }
}
