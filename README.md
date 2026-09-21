# caveat

A rule in a `CLAUDE.md` is a thing to remember. A lesson is needed when its
error message is on the screen, not two thousand lines earlier — by then
nobody rereads the file. `caveat` is a Claude Code hook over a directory of
Markdown files, one lesson each. It watches every `Bash` call: the command
about to run, and what it printed afterwards. When one matches a known
pitfall of the repository, the lesson is injected as context, once per
session — read by the model, at the moment it would otherwise repeat the
mistake.

## The file format

A caveat is `caveats/<slug>.md`: YAML frontmatter, then the lesson as
Markdown.

```markdown
---
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
```

`title` and `line` are required and must not be empty; `since` and `family`
are free text, for a human reading the directory. An unknown key is an
error — a typo must not silently be ignored.

`output` triggers match what a call printed, `command` triggers match the
command before it runs.

An output trigger needs exactly one of:

- `text` — a literal, case-sensitive substring. The default: this project
  has paid enough for patterns that looked right and matched the wrong
  line.
- `regex` — a regular expression.

A command trigger has no `text`/`regex` of its own; instead it takes:

- `program` — matches only at command position, behind `sudo`, `nice`,
  `exec`, `command`, `timeout …`, and `lotse run --class=… --`, and behind
  a path (`/usr/bin/ssh` still matches `ssh`). Only the wrapper's NAME is
  seen through, not its own options: `sudo -u x ssh` and `nice -n 10 ssh`
  do not match `program: ssh` — the option word (`-u`, `-n`) is taken for
  the program name instead.
- `pattern` — a regex the matched command segment must also satisfy.
- `where: position` (the default) — only where the shell would actually run
  a command: not inside a quoted string, not as an ARGUMENT of another
  command (`echo ssh`, `xargs ssh`, `bash -c 'ssh …'`). Each side of a pipe
  or `&&`/`;` IS its own command position (`ls | ssh host` matches). Needs
  `program`; `pattern` is then optional on top of it.
- `where: anywhere` — anywhere in the raw line, quoted or not. This is what
  sees the inner command of `ssh host '…'` or `bash -c '…'`, which `where:
  position` cannot: it has no program of its own to match, and even
  `pattern` alone only ever looks at what the local shell would run. Needs
  `pattern`; `program` is then forbidden, not merely unneeded — a command
  trigger cannot combine a program name with a plain textual search.

The scanner behind `where: position` is conservative: what it does not
understand matches nothing there (`where: anywhere` still does, since it
never parses the line). This includes a here-document, a backtick, `$((`,
`[[ … ]]`, `case … esac`, an array literal (`x=(…`, `x+=(…`), `$'…'`
(ANSI-C quoting), and an unclosed quote. A `\` line continuation is not
understood either, in a weaker way: it does not make the whole trigger
fail, but a pattern only ever sees the FIRST line — nothing past the `\`
is there to match.

`hits` and `misses` are required on every trigger: one example the trigger
must match, one it must not. They are never executed — `caveat check` only
runs the trigger's own matcher against the two strings, plus a shared list
of everyday lines in `caveats/harmless.txt` that no trigger is allowed to
match (`git status`, `ssh -F ssh_config vps`, and the like). Blank lines
and lines starting with `#` in `harmless.txt` are ignored. Without that
file, `check` reports it missing: a trigger too broad to notice otherwise
goes unnoticed.

## Commands

- **`caveat hook claude`** — reads one Claude Code hook event from stdin,
  writes a reply to stdout if a fresh caveat matches, otherwise nothing.
  Always exits `0`: whatever goes wrong is a line in
  `$XDG_STATE_HOME/caveat/hook.log`, never a hung tool call. The default
  state directory, when `XDG_STATE_HOME` is unset or empty, is
  `~/.local/state/caveat`; with neither `XDG_STATE_HOME` nor `HOME` giving
  an absolute path there is no state directory at all — the hook still
  replies, it just keeps no log and no per-session memory of what it
  already showed.
- **`caveat check [--dir DIR]`** — runs the self-test (below), then checks
  every trigger of every file in `DIR` (or the nearest `caveats/` upwards
  from the current directory) against its `hits`, its `misses`, and
  `harmless.txt`. A file that fails to parse is a finding too, not a tool
  error — it does not take the others down with it. Exit `0` if every
  trigger holds, `1` if it printed a finding, `2` on a tool error — no
  `caveats` directory found, the directory is empty (a check over nothing
  says nothing), or a self-test that no longer passes.
- **`caveat search [--dir DIR] TEXT…`** — ranks caveats against `TEXT`:
  highest if a trigger matches it, then a title match, then a body match.
  Paste the error message itself; the triggers then work the right way
  round. Exit `0` if it printed at least one, `1` if it found nothing, `2`
  on a tool error — no query given, or no `caveats` directory found.

## Installing the hook

In `~/.claude/settings.json`, under all three events:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [
        { "type": "command", "command": "caveat hook claude", "timeout": 5 }
      ] }
    ],
    "PostToolUse": [
      { "matcher": "Bash", "hooks": [
        { "type": "command", "command": "caveat hook claude", "timeout": 5 }
      ] }
    ],
    "PostToolUseFailure": [
      { "matcher": "Bash", "hooks": [
        { "type": "command", "command": "caveat hook claude", "timeout": 5 }
      ] }
    ]
  }
}
```

`PreToolUse` matches `command` triggers against the command about to run;
`PostToolUse` and `PostToolUseFailure` match `output` triggers against what
it printed. Without a `caveats` directory upwards from the event's `cwd`
(and no fallback `dir = "…"` in `$XDG_CONFIG_HOME/caveat/config.toml`), the
hook does nothing.

**The trust model in one sentence:** a `caveats/` directory found above the
`cwd` is trusted the way a `CLAUDE.md` found there is — its text (the body
of a matching caveat) goes straight to the model as context, unreviewed at
that point, the same as any other file in the repository that ends up read
by an agent working in it.

## What it does not do

- **Never rewrites a command.** Several `PreToolUse` hooks run in parallel,
  each sees the original command, and only ONE `updatedInput` among all of
  them wins, non-deterministically — a hook that means to change the
  command cannot rely on being the one that does.
- **Never decides a permission.** It never sets `permissionDecision`. A
  caveat shown before a call stops nothing: the command still runs. Whoever
  wants a barrier builds it elsewhere.

## Limits

Measured against Claude Code's actual hook input, not assumed from its
documentation:

- On success, `tool_response.stdout` is cut to its first 30,000 characters;
  the full output lies in the file named by
  `tool_response.persistedOutputPath`. The hook reads the last 8 MiB of
  that file, so an `output` trigger sees the tail even when `stdout` alone
  would have missed it.
- On failure there is no `tool_response`, only `error` — and `error` itself
  is cut in the *middle*: the beginning, then
  `... [N characters truncated] ...`, then the end of the first 30,000
  characters of the original output. **The true end of a long failing
  output is invisible to the hook.** An error message usually stands at the
  end. Piping a long command through `tail` before it can fail is the
  practical way around this.
- A caveat is shown at most once per session — and a subagent counts as its
  own session: it starts with a fresh context and has not read what its
  parent was shown. Kept in `$XDG_STATE_HOME/caveat/seen/`, one file per
  session and agent, next to `hook.log`.
- A reply carries at most 2 full caveats (title, path, body, cut at 6,000
  characters with a pointer to the file); a third and later match is named
  by title and path only.
- Printing a caveat triggers it like any other output: `cat` over a lesson
  file, a `grep` across `caveats/`, a red `caveat check` — each costs one
  injection. Accepted, not worked around.

## The self-test

`caveat check` first runs a self-test: five events recorded from a real
Claude Code run (`tests/recorded/*.json`, one per kind — `PreToolUse`,
`PostToolUse`, a failure, and the large and truncated variants of the
latter two) are fed to the same `respond()` that the hook calls, against
five built-in probe caveats, and each must still produce a match. The
events are recorded, not rebuilt from the code's own idea of the format: a
fixture written from the same understanding as the code cannot contradict
it, and Claude Code's actual JSON shape can. A failing self-test means the
hook has silently stopped seeing something it used to see — checked before
every `caveat check`, and exercised by `nix flake check` through `cargo
test`.

## Not yet

An index generator over a `caveats` directory, and a hit counter reading
`hook.log`, are planned but do not exist.

## License

AGPL-3.0-only.
