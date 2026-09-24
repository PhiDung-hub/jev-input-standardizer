---
name: jev-input-standardizer
description: Use, inspect, configure, or troubleshoot the local Jev composer standardizer for Codex and Claude Code (plain text for one kind of content; Markdown for mixed GPT prompts, XML for mixed Claude prompts; user-chosen formats). Use for Alt+G review, format keys, JEV_STANDARDIZER_FORMAT, bounded typo and question edits, research flags, standardizer_notes, context hooks, encoding, bridge health, or latency.
---

# Jev Input Standardizer

The shared loopback bridge defaults to `http://127.0.0.1:8788`. Its TypeSafe
credential stays in the service environment; never copy it into a prompt,
request payload, plugin file, or preview.

- Read `/health` before diagnosing the editor bridge. `usage_exhausted` means further
  Jev calls are suppressed until the server restarts after usage is replenished.
- POST `{"context": ..., "options": ...}` to `/standardize` when an application
  needs the actual downstream payload. Send the response's `text` field to its
  LLM; `alternatives` holds the plain, XML, Markdown, and JSON renderings.
- Prose of one kind of content (every role Jev confirmed the same) goes out as plain
  text: the user's words with approved edits, in their layout, notes in a small
  XML block after them. A part whose role Jev did not confirm stays untagged
  (`"role": null` in JSON). Mixed prose uses the target's format: Markdown `##`
  sections for GPT/Codex targets, XML tags for Claude (the model name decides,
  the harness is the fallback). `JEV_STANDARDIZER_FORMAT` or `options.format`
  overrides it; `encodingDecision.reason` says which rule applied. Structured
  data still gets Jev's JSON/TOON/CSV/XML choice.
- The editor sends the harness id (`host`), the session's model
  (`targetModel`), and `background`: up to twelve recent conversation turns
  (24,000 characters; `JEV_STANDARDIZER_CONTEXT_CHARS=0` disables them) plus
  items from the optional `JEV_STANDARDIZER_CONTEXT_CMD` hook (draft on stdin;
  JSON lines `{"source","text"}` or plain text). Turns inform Jev but are never
  quoted, since the harness has them; of the hook items, Jev names the one the
  draft depends on most and at most two are quoted (clipped) into
  `standardizer_notes.earlier_context`. The bridge never logs them.
- New harnesses are a row in `src/harness.rs` plus a transcript reader; new
  model vendors are a row in `src/target.rs`.
- Filler removal covers politeness only; `just`, `if possible`, `actually`,
  `simply`, and `can/could/would you` are kept because they can carry intent.
- Alt+G on an accepted payload restores the original draft (kept per session in
  `$XDG_RUNTIME_DIR`) plus any appended text before standardizing again.
- For plain text, code enumerates source-preserving candidate boundaries and Jev
  decides split versus merge. Boundary, role, filler, edit, answer-style,
  research-flag, and context questions run
  together in the same fan-out request; each applies only at `minConfidence`
  (0.8), and anything less keeps the user's layout and wording. Code proposes bounded typo, idiom,
  and question casing/punctuation edits; Jev chooses among them and the user
  reviews the result. Jev does not generate replacement prose.
- The research choice only instructs the downstream Codex or Claude agent to
  inspect local material or verify current sources. Alt+G does not fetch them.
- In a normally launched Codex or Claude Code session, draft a prompt and press
  Alt+G. The external-editor bridge asks Jev and shows a short review screen
  (header, exact prompt, one line of changes and additions). `p`, `x`, `m`,
  and `j` switch formats without a new request; `y` writes the shown prompt
  back and `n` keeps the draft, with no Enter needed. The
  `jev-codex` and `jev-claude` launchers remain available as explicit fallbacks.
- The accepted payload returns to the host composer. Codex and Claude Code do
  not expose a supported external-editor action that submits it, so after `y`
  the user presses Enter once more. Inside WezTerm, `s` accepts and sends: it
  presses Enter in the pane via `wezterm cli send-text` 0.4 s after the editor
  exits. Never claim that `y` auto-submits.
- The server uses one Jev fan-out call, no retry, and one identical backup
  request only when the first has not answered within `hedgeAfterMs` (700 ms);
  the first answer wins and the overall deadline is unchanged. It keeps its Jev
  connection warm, has no result cache, and falls back to source layout when a
  boundary answer is missing.
- `/health` includes process-lifetime usage counters. For per-request metadata
  without prompt contents, inspect `journalctl --user -u
  jev-input-standardizer.service`; never add raw context to service logs.
- On a bridge or Jev failure the screen shows the error and `r` retries; any
  other key, or rejection, leaves the original composer draft unchanged.

Implementation and configuration details are in
<https://github.com/PhiDung-hub/jev-input-standardizer#readme>.
