//! `caveat check`: can every trigger turn red, and does none fire on everyday lines?

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::{hook, store};

pub struct Finding {
    pub path: PathBuf,
    pub message: String,
}

/// One trigger against its two examples and the harmless lines.
fn examine(
    path: &Path,
    label: &str,
    hits: &str,
    misses: &str,
    harmless: &[String],
    matches: &dyn Fn(&str) -> Result<bool>,
    out: &mut Vec<Finding>,
) {
    let mut push = |message: String| {
        out.push(Finding {
            path: path.to_path_buf(),
            message,
        })
    };
    match matches(hits) {
        Err(error) => return push(format!("{label}: {error:#}")),
        Ok(false) => push(format!("{label} does not match its `hits`: {hits:?}")),
        Ok(true) => {}
    }
    if matches(misses).unwrap_or(false) {
        push(format!("{label} matches its `misses`: {misses:?}"));
    }
    for line in harmless {
        if matches(line).unwrap_or(false) {
            push(format!("{label} matches a line of harmless.txt: {line:?}"));
        }
    }
}

pub fn check(dir: &Path) -> Result<Vec<Finding>> {
    hook::self_test()?;
    let (entries, broken) = store::load(dir)?;
    if entries.is_empty() && broken.is_empty() {
        bail!(
            "no caveats in {}: a check over nothing says nothing",
            dir.display()
        );
    }
    let mut findings: Vec<Finding> = broken
        .into_iter()
        .map(|(path, error)| Finding {
            path,
            message: format!("{error:#}"),
        })
        .collect();
    let harmless_path = dir.join("harmless.txt");
    let harmless: Vec<String> = match std::fs::read_to_string(&harmless_path) {
        Ok(raw) => raw
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect(),
        Err(_) => {
            findings.push(Finding {
                path: harmless_path,
                message:
                    "missing: without everyday lines, a trigger that is too broad goes unnoticed"
                        .into(),
            });
            Vec::new()
        }
    };
    for entry in &entries {
        for (i, t) in entry.output.iter().enumerate() {
            let label = format!("output trigger {}", i + 1);
            examine(
                &entry.path,
                &label,
                &t.hits,
                &t.misses,
                &harmless,
                &|s| t.matches(s),
                &mut findings,
            );
        }
        for (i, t) in entry.command.iter().enumerate() {
            let label = format!("command trigger {}", i + 1);
            examine(
                &entry.path,
                &label,
                &t.hits,
                &t.misses,
                &harmless,
                &|s| t.matches(s),
                &mut findings,
            );
        }
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for (name, content) in files {
            fs::write(tmp.path().join(name), content).unwrap();
        }
        tmp
    }

    const GOOD: &str = "---\ntitle: T\nline: L\noutput:\n  - text: \"Argument list too long\"\n    hits: \"printf: Argument list too long\"\n    misses: \"argument list\"\ncommand:\n  - program: ssh\n    pattern: '203\\.0\\.113'\n    hits: \"ssh root@203.0.113.7\"\n    misses: \"ssh vps\"\n---\nB\n";

    #[test]
    fn a_good_directory_has_no_findings() {
        let tmp = dir_with(&[
            ("a.md", GOOD),
            (
                "harmless.txt",
                "# everyday\ngit status\n\nssh -F ssh_config vps\n",
            ),
        ]);
        assert!(check(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn a_trigger_that_misses_its_own_example_is_a_finding() {
        let bad = GOOD.replace("printf: Argument list too long", "printf: all fine");
        let tmp = dir_with(&[("a.md", &bad), ("harmless.txt", "git status\n")]);
        let found = check(tmp.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("does not match its `hits`"));
    }

    #[test]
    fn a_trigger_that_matches_its_counterexample_is_a_finding() {
        let bad = GOOD.replace("misses: \"ssh vps\"", "misses: \"ssh root@203.0.113.99\"");
        let tmp = dir_with(&[("a.md", &bad), ("harmless.txt", "git status\n")]);
        assert!(
            check(tmp.path()).unwrap()[0]
                .message
                .contains("matches its `misses`")
        );
    }

    #[test]
    fn a_trigger_that_matches_a_harmless_line_is_a_finding() {
        let tmp = dir_with(&[
            ("a.md", GOOD),
            ("harmless.txt", "ssh root@203.0.113.7 uptime\n"),
        ]);
        assert!(
            check(tmp.path()).unwrap()[0]
                .message
                .contains("harmless.txt")
        );
    }

    #[test]
    fn a_broken_file_a_broken_regex_and_a_missing_harmless_file_are_findings() {
        let regex = "---\ntitle: T\nline: L\noutput:\n  - regex: \"(\"\n    hits: a\n    misses: b\n---\nB\n";
        let tmp = dir_with(&[("a.md", GOOD), ("b.md", "no frontmatter"), ("c.md", regex)]);
        let found = check(tmp.path()).unwrap();
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn an_empty_directory_is_a_tool_error_not_a_pass() {
        let tmp = dir_with(&[("harmless.txt", "git status\n")]);
        assert!(check(tmp.path()).is_err());
    }
}
