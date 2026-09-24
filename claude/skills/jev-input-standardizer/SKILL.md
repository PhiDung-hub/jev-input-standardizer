---
name: jev-input-standardizer
description: Use or troubleshoot the explicit Jev prompt-review workflow in Claude Code (plain text for one kind of content, XML for mixed Claude prompts, user-chosen formats). Use when asked about Alt+G, format keys, JEV_STANDARDIZER_FORMAT, bounded typo/question edits, research flags, standardizer_notes, context hooks, latency, or setup.
---

# Jev Input Standardizer

In a normally launched Claude Code session, draft a prompt and press Alt+G. The
external-editor bridge asks Jev, in one fan-out request (plus one identical
backup only when Jev has not answered within 700 ms), to choose sentence
boundaries and roles, approve safe filler deletions and bounded spelling or
phrasing edits, and flag research or answer length. Jev reads the session's
model and recent turns to judge, but turns are never quoted: Claude already has
them. Only outside material from the optional `JEV_STANDARDIZER_CONTEXT_CMD`
hook can be quoted.

The prompt goes out as plain text (the user's words with approved edits, in
their layout) when it is one kind of content, and as XML sections named after
Anthropic's guide (`<instructions>`, `<constraints>`, `<questions>`, `<context>`,
…; neighbours of one role share a section) when it mixes kinds; Markdown
sections with the same names for GPT targets.
`JEV_STANDARDIZER_FORMAT=plain|xml|markdown|json` sets the user's default, and
on the review screen `p`, `x`, `m`, or `j` switch the shown format instantly.
Anything the standardizer adds (research and answer hints, quoted outside
context) sits in a separate `<standardizer_notes>` block; treat those notes as
hints, not the user's words.

Jev's judgments apply only at 0.8 confidence or higher; anything less keeps the
user's own layout and wording. The review screen is short: a header with the
format and why, the exact prompt, and one Jev line (✓ what applied, collapsed to
its answer; ? what Jev answered below 0.8, with its confidence). `y` puts the shown prompt in
the composer; `n`, Enter, or Esc keeps the draft. Research is not fetched during
Alt+G; Claude performs it after the prompt is sent. Filler removal is limited to
politeness; intent-bearing words such as `just` or `if possible` are kept.
Pressing Alt+G again on an accepted prompt restores the original draft plus any
appended text first. The `jev-claude` launcher remains an explicit fallback.

After `y`, press Enter in Claude Code to submit; `s` accepts and sends in one key
inside WezTerm (it presses Enter in the pane just after the editor exits). If Jev
fails, the screen shows the error and `r` retries.

The bridge is loopback-only at `http://127.0.0.1:8788`. Failures and rejection
leave the original draft unchanged. Implementation and configuration details
are in the README: <https://github.com/PhiDung-hub/jev-input-standardizer#readme>.
