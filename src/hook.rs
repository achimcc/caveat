//! `caveat hook claude`: from a Claude Code hook event to what the model is shown.
//!
//! This hook never rewrites a command and never decides a permission. Several
//! PreToolUse hooks run in parallel and only ONE `updatedInput` wins; lotse
//! needs that slot. `additionalContext` of every hook is kept.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::entry::{Entry, parse};
use crate::seen::Seen;
use crate::trigger::Scanned;

/// A body longer than this is cut, with a pointer to the file.
pub const CAP: usize = 6000;
/// More caveats than this in one call come as title and path only.
pub const FULL: usize = 2;
/// How much of a persisted output is read, from its end.
pub const TAIL: u64 = 8 * 1024 * 1024;

const PRE: &str = include_str!("../tests/recorded/pre_tool_use.json");
const POST: &str = include_str!("../tests/recorded/post_tool_use.json");
const FAILURE: &str = include_str!("../tests/recorded/post_tool_use_failure.json");
const POST_LARGE: &str = include_str!("../tests/recorded/post_tool_use_large.json");
const FAILURE_LARGE: &str = include_str!("../tests/recorded/post_tool_use_failure_large.json");

pub struct Reply {
    pub json: Value,
    pub slugs: Vec<String>,
}

fn tail_of(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// What the call printed, as far as the event carries it. On success the
/// output is cut to its first 30000 characters and the whole of it lies in
/// `persistedOutputPath`; on failure there is only `error`, cut in the middle.
fn printed(event: &Value) -> Option<String> {
    if let Some(error) = event["error"].as_str() {
        return Some(error.to_string());
    }
    let response = event.get("tool_response")?;
    let mut text = String::new();
    for key in ["stdout", "stderr"] {
        if let Some(part) = response[key].as_str() {
            text.push_str(part);
            text.push('\n');
        }
    }
    if let Some(tail) = response["persistedOutputPath"]
        .as_str()
        .and_then(|p| tail_of(Path::new(p)))
    {
        text.push_str(&tail);
    }
    Some(text)
}

fn cut(body: &str, path: &Path) -> String {
    match body.char_indices().nth(CAP) {
        None => body.to_string(),
        Some((at, _)) => format!(
            "{}\n\n[cut at {CAP} characters: read {}]",
            &body[..at],
            path.display()
        ),
    }
}

pub fn respond(entries: &[Entry], event: &Value, seen: &mut Seen) -> Option<Reply> {
    if event["tool_name"] != "Bash" {
        return None;
    }
    let name = event["hook_event_name"].as_str()?;
    // A trigger that does not compile matches nothing here; `caveat check` reports it.
    let hits: Vec<&Entry> = match name {
        "PreToolUse" => {
            let command = event["tool_input"]["command"].as_str()?;
            // Scanned once per hook call and shared by every trigger of
            // every caveat: `matches_lazily` checks `scan::program` against
            // this scan before it ever compiles a `where: position`
            // pattern, so a caveat whose `program` is not even in the
            // command never pays for a `Regex::new`.
            let scanned = Scanned::of(command);
            entries
                .iter()
                .filter(|e| e.command.iter().any(|t| t.matches_lazily(&scanned)))
                .collect()
        }
        "PostToolUse" | "PostToolUseFailure" => {
            let text = printed(event)?;
            entries
                .iter()
                .filter(|e| e.output.iter().any(|t| t.matches(&text).unwrap_or(false)))
                .collect()
        }
        _ => return None,
    };
    let fresh: Vec<&Entry> = hits
        .into_iter()
        .filter(|e| !seen.contains(&e.slug))
        .collect();
    if fresh.is_empty() {
        return None;
    }
    let mut text = String::from(
        "caveat: this call matches a known pitfall of this repository. \
         Read it before you go on; it is shown once per session.\n",
    );
    for (i, entry) in fresh.iter().enumerate() {
        if i < FULL {
            text.push_str(&format!(
                "\n## {}\n({})\n\n{}\n",
                entry.title,
                entry.path.display(),
                cut(&entry.body, &entry.path)
            ));
        } else {
            text.push_str(&format!(
                "\nAlso matching: {} ({})\n",
                entry.title,
                entry.path.display()
            ));
        }
        seen.add(&entry.slug);
    }
    Some(Reply {
        json: json!({ "hookSpecificOutput": { "hookEventName": name, "additionalContext": text } }),
        slugs: fresh.iter().map(|e| e.slug.clone()).collect(),
    })
}

/// The caveats of the self-test: one per kind of event.
pub fn probes() -> Vec<Entry> {
    let make = |slug: &str, triggers: &str, body: &str| {
        parse(
            Path::new(&format!("{slug}.md")),
            &format!("---\ntitle: Probe {slug}\nline: l\n{triggers}---\n{body}\n"),
        )
        .expect("the built-in probes parse")
    };
    vec![
        make(
            "probe-command",
            "command:\n  - program: echo\n    pattern: spike-erfolg\n    hits: echo spike-erfolg\n    misses: echo x\n",
            "BODY-COMMAND",
        ),
        make(
            "probe-output",
            "output:\n  - text: spike-erfolg\n    hits: spike-erfolg\n    misses: x\n",
            "BODY-OUTPUT",
        ),
        make(
            "probe-failure",
            "output:\n  - text: gibt-es-nicht-spike\n    hits: gibt-es-nicht-spike\n    misses: x\n",
            "BODY-FAILURE",
        ),
        make(
            "probe-large",
            "output:\n  - text: \"\\n6000\\n\"\n    hits: \"5999\\n6000\\n6001\"\n    misses: x\n",
            "BODY-LARGE",
        ),
        make(
            "probe-truncated",
            "output:\n  - text: characters truncated\n    hits: \"[20012 characters truncated]\"\n    misses: x\n",
            "BODY-TRUNCATED",
        ),
    ]
}

/// The positive control: events RECORDED from a real Claude Code run must
/// still produce a reply. Recorded, not rebuilt: a fixture written from the
/// same understanding as the code cannot contradict it.
pub fn self_test() -> Result<()> {
    let cases = [
        ("pre_tool_use", PRE, "probe-command"),
        ("post_tool_use", POST, "probe-output"),
        ("post_tool_use_failure", FAILURE, "probe-failure"),
        ("post_tool_use_large", POST_LARGE, "probe-large"),
        (
            "post_tool_use_failure_large",
            FAILURE_LARGE,
            "probe-truncated",
        ),
    ];
    let probes = probes();
    for (name, raw, slug) in cases {
        let event: Value = serde_json::from_str(raw)?;
        let reply = respond(&probes, &event, &mut Seen::memory());
        let shown = reply.map(|r| r.slugs).unwrap_or_default();
        if !shown.iter().any(|s| s == slug) {
            bail!(
                "self-test: the recorded event `{name}` no longer yields `{slug}` (got {shown:?})"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn event(raw: &str) -> Value {
        serde_json::from_str(raw).unwrap()
    }

    fn context(reply: &Reply) -> &str {
        reply.json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
    }

    #[test]
    fn the_recorded_events_still_produce_a_reply() {
        self_test().unwrap();
    }

    #[test]
    fn a_pre_tool_use_reply_names_its_event_and_approves_nothing() {
        let reply = respond(&probes(), &event(PRE), &mut Seen::memory()).unwrap();
        let out = &reply.json["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], "PreToolUse");
        assert!(out.get("permissionDecision").is_none());
        assert!(out.get("updatedInput").is_none());
        assert_eq!(reply.slugs, ["probe-command"]);
        assert!(context(&reply).contains("BODY-COMMAND"));
        assert!(context(&reply).contains("probe-command.md"));
    }

    #[test]
    fn output_is_only_matched_after_the_call_and_commands_only_before() {
        let only_output: Vec<Entry> = probes()
            .into_iter()
            .filter(|e| e.slug == "probe-output")
            .collect();
        assert!(respond(&only_output, &event(PRE), &mut Seen::memory()).is_none());
        let only_command: Vec<Entry> = probes()
            .into_iter()
            .filter(|e| e.slug == "probe-command")
            .collect();
        assert!(respond(&only_command, &event(POST), &mut Seen::memory()).is_none());
    }

    #[test]
    fn a_failure_is_read_from_its_error_field() {
        let reply = respond(&probes(), &event(FAILURE), &mut Seen::memory()).unwrap();
        assert_eq!(
            reply.json["hookSpecificOutput"]["hookEventName"],
            "PostToolUseFailure"
        );
        assert_eq!(reply.slugs, ["probe-failure"]);
    }

    #[test]
    fn a_caveat_is_shown_once() {
        let mut seen = Seen::memory();
        assert!(respond(&probes(), &event(POST), &mut seen).is_some());
        assert!(respond(&probes(), &event(POST), &mut seen).is_none());
    }

    #[test]
    fn a_persisted_output_is_read_from_its_end_and_a_missing_one_is_no_error() {
        // On the in-memory copy, point `persistedOutputPath` at a path
        // that provably does not exist (a name never written inside a
        // fresh tempdir) — not at whatever the recording happened to
        // leave behind, which may or may not still be there depending on
        // the machine. The truncated stdout still matches on its own.
        let mut large = event(POST_LARGE);
        let missing_dir = tempfile::tempdir().unwrap();
        large["tool_response"]["persistedOutputPath"] =
            Value::String(missing_dir.path().join("gone.txt").display().to_string());
        assert!(respond(&probes(), &large, &mut Seen::memory()).is_some());
        // With the file in place, the END of the output is visible, which stdout lost.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "1\n2\nEND-MARKER-OF-THE-RUN\n").unwrap();
        large["tool_response"]["persistedOutputPath"] =
            Value::String(tmp.path().display().to_string());
        let end = parse(
            std::path::Path::new("end.md"),
            "---\ntitle: End\nline: l\noutput:\n  - text: END-MARKER-OF-THE-RUN\n    hits: END-MARKER-OF-THE-RUN\n    misses: x\n---\nBODY-END\n",
        )
        .unwrap();
        let reply = respond(&[end], &large, &mut Seen::memory()).unwrap();
        assert_eq!(reply.slugs, ["end"]);
    }

    #[test]
    fn a_long_body_is_capped_and_the_third_caveat_comes_as_a_title() {
        let body = "x".repeat(CAP + 500);
        let make = |slug: &str| {
            parse(
                std::path::Path::new(&format!("{slug}.md")),
                &format!("---\ntitle: Title {slug}\nline: l\noutput:\n  - text: spike-erfolg\n    hits: spike-erfolg\n    misses: x\n---\n{body}\n"),
            )
            .unwrap()
        };
        let entries = [make("a"), make("b"), make("c")];
        let reply = respond(&entries, &event(POST), &mut Seen::memory()).unwrap();
        let text = context(&reply);
        assert!(text.len() < 2 * (CAP + 600));
        assert!(text.contains("Title c"));
        assert_eq!(text.matches("[cut at").count(), 2);
        assert_eq!(reply.slugs, ["a", "b", "c"]);
    }

    #[test]
    fn a_broken_regex_next_to_a_matching_text_trigger_still_replies() {
        let entry = parse(
            std::path::Path::new("mixed.md"),
            "---\ntitle: Mixed\nline: l\noutput:\n  - regex: \"(\"\n    hits: a\n    misses: b\n  - text: spike-erfolg\n    hits: spike-erfolg\n    misses: x\n---\nBODY-MIXED\n",
        )
        .unwrap();
        let reply = respond(&[entry], &event(POST), &mut Seen::memory()).unwrap();
        assert_eq!(reply.slugs, ["mixed"]);
        assert!(context(&reply).contains("BODY-MIXED"));
    }

    #[test]
    fn other_tools_other_events_and_broken_triggers_say_nothing() {
        let mut other_tool = event(POST);
        other_tool["tool_name"] = Value::String("Read".into());
        assert!(respond(&probes(), &other_tool, &mut Seen::memory()).is_none());
        let mut other_event = event(POST);
        other_event["hook_event_name"] = Value::String("Stop".into());
        assert!(respond(&probes(), &other_event, &mut Seen::memory()).is_none());
        let broken = parse(
            std::path::Path::new("broken.md"),
            "---\ntitle: T\nline: l\noutput:\n  - regex: \"(\"\n    hits: a\n    misses: b\n---\nB\n",
        )
        .unwrap();
        assert!(respond(&[broken], &event(POST), &mut Seen::memory()).is_none());
    }
}
