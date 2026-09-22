//! `caveat search`: for a human, or a session that wants to look something up.

use crate::entry::Entry;

fn rank(entry: &Entry, query: &str, lower: &str) -> u8 {
    let by_trigger = entry
        .output
        .iter()
        .any(|t| t.compile().is_ok_and(|m| m.matches(query)))
        || entry
            .command
            .iter()
            .any(|t| t.compile().is_ok_and(|m| m.matches(query)));
    if by_trigger {
        3
    } else if entry.title.to_lowercase().contains(lower) {
        2
    } else if entry.body.to_lowercase().contains(lower) {
        1
    } else {
        0
    }
}

/// Best first. Paste the ERROR MESSAGE: the triggers then work the right way round.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let lower = query.to_lowercase();
    let mut ranked: Vec<(u8, &Entry)> = entries
        .iter()
        .map(|e| (rank(e, query, &lower), e))
        .filter(|(r, _)| *r > 0)
        .collect();
    ranked.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
    ranked.into_iter().map(|(_, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::parse;
    use std::path::Path;

    fn entries() -> Vec<Entry> {
        vec![
            parse(Path::new("body.md"), "---\ntitle: Something else\nline: l\n---\nmentions Namespacing in passing\n").unwrap(),
            parse(Path::new("trigger.md"), "---\ntitle: Capabilities\nline: l\noutput:\n  - text: \"Failed to set up user namespacing\"\n    hits: a\n    misses: b\n---\nB\n").unwrap(),
            parse(Path::new("title.md"), "---\ntitle: About namespacing\nline: l\n---\nB\n").unwrap(),
        ]
    }

    #[test]
    fn a_pasted_message_finds_its_caveat_first() {
        let all = entries();
        let found = search(
            &all,
            "foo.service: Failed to set up user namespacing: No such file",
        );
        assert_eq!(found[0].slug, "trigger");
    }

    #[test]
    fn title_ranks_above_body_and_case_does_not_count() {
        let all = entries();
        let slugs: Vec<&str> = search(&all, "NAMESPACING")
            .iter()
            .map(|e| e.slug.as_str())
            .collect();
        assert_eq!(slugs, ["title", "body"]);
    }

    #[test]
    fn nothing_is_nothing() {
        assert!(search(&entries(), "zebra").is_empty());
    }
}
