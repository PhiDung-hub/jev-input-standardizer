# Jev Input Standardizer

A small Rust package that lets Codex and Claude Code users explicitly convert a
composer draft into a clean prompt before sending it: plain text when the draft
is one kind of content, the target model's prompt format (XML tags for Claude,
Markdown sections for GPT) when it mixes kinds, or whatever format you pick.
Structured data can still use JSON, TOON, or CSV.

The workflow is the same in both hosts:

1. Start Codex or Claude Code normally.
2. Write a normal prompt and press `Alt+G`.
3. Jev chooses semantic sentence boundaries, classifies the resulting spans,
   reviews bounded typo and phrasing candidates, flags whether the downstream
   agent should research, and names the background item the draft depends on,
   in one fan-out request. It sees the harness, the target model, the last
   twelve conversation turns, and anything a context hook adds. Code then
   renders the prompt.
4. Review the exact prompt on a short screen: a header, the prompt, and one
   Jev line (what applied, and what Jev answered below 0.8 confidence that would
   have changed the prompt). Press `p`, `x`, `m`, or `j` to see it as plain text,
   XML, Markdown, or JSON (no new request), then `y` to put the shown prompt in
   the composer, `s` (in WezTerm) to put it there and send it, or `n` to keep
   the draft. No Enter is needed. If Jev fails, `r` retries.
5. After `y`, press Enter in the host to send it.

The local host configuration maps each external-editor action to `Alt+G`,
leaving `Ctrl+G` free for Nano. Shell-scoped environment and Claude's user
settings select the Jev editor only for agent sessions, so the user's normal
editor is unchanged. Neither host exposes a supported way for an external
editor to submit the returned draft, so the final Enter stays under user
control: `y` leaves it to you, and `s` (WezTerm only) presses it for you. No
automatic prompt hook runs.

## Behavior

The input is JSON-shaped context. For a plain-text root prompt, code enumerates
boundary candidates at sentence endings, meaningful line breaks, and a
comma followed by an explicit follow-up question. Jev makes only bounded
judgments:

- a Noul deciding split versus merge for each candidate boundary;
- a Choice for each segment role;
- a Noul for each detected filler phrase;
- a Choice for each bounded spelling, question, or phrasing candidate;
- Choices for desired answer length and whether Codex or Claude should inspect
  local files or verify current external facts;
- one Choice naming the background item the draft depends on most, or none;
  and
- for structured (non-prose) input only, one Choice between the available
  JSON, TOON, CSV, and XML candidates.

These independent judgments share one state and run in one fan-out request.
The editor also tells Jev which harness the draft is going to, which model that
session uses, and background: up to twelve recent conversation turns (each
clipped to 2,000 characters, 24,000 in total) plus any context-hook items. It reads them from the
host's own session file: the Claude Code transcript of the owning Claude
process (found through `~/.claude/sessions/<pid>.json`, else the newest
transcript for the directory), or the Codex rollout named by `CODEX_THREAD_ID`
(else the newest rollout for the current directory, else the `model` in
`~/.codex/config.toml`). Only the last 4 MiB of a transcript is read.
Conversation turns inform Jev's judgments but are never quoted: the harness
already has them. Only outside material (context-hook items) can be quoted:
Jev names the item the draft depends on most (a single comparative Choice; a
Noul per item scored nearly everything 0.55–0.8 in live use), and an item at
`minConfidence` or more goes, clipped to its first and last
300 characters, into `standardizer_notes.earlier_context` as `[source] text`.
Without a hook, Jev is not asked. The bridge never logs background text. Set `JEV_STANDARDIZER_CONTEXT_CHARS=0` to send the model name only.

A context hook adds your own background. `JEV_STANDARDIZER_CONTEXT_CMD` runs
through `sh -c` in the session's directory with a 2-second deadline, the draft
on stdin, and `JEV_STANDARDIZER_TARGET_MODEL` set. Each stdout line
`{"source": "notes", "text": "..."}` is one item; any other output is one
`custom` item. For example, to offer the project's AGENTS.md:

```sh
export JEV_STANDARDIZER_CONTEXT_CMD='test -f AGENTS.md && jq -cRs "{source: \"agents_md\", text: .}" AGENTS.md'
```
Possible skill invocations (`$web-design-guidelines` in Codex or `/deploy` in
Claude Code) are passed as host context and preserved verbatim. Auto mode
avoids CSV for such drafts so the skill reference remains obvious in the
prompt. This is recognition of a possible
invocation, not verification that the skill is installed in that host.
Claude Code commands at the very start of a draft are left entirely unchanged:
its slash-command parser requires that position, so wrapping one in a structured
payload would prevent the command from running.
Every Jev judgment applies only at `minConfidence` (0.8 by default): a Choice's
top answer at or above it, a Noul at or above it (yes) or at or below 0.2 (no).
Anything less falls back to the user's own layout and wording (a split keeps the
source's break, a role its heuristic, an edit or filler deletion is skipped),
and still shows on the review screen. There is no second segmentation or
encoding call.
An action and its follow-up question can therefore become separate `task` and
`question` segments. `[Image #1]`-style attachment markers stay verbatim; the
harness attaches the image itself, so no note is added.

CSV is generated only when the standardized value is a uniform array of
records (or a single wrapper field containing one) and every cell is scalar.
Irregular or nested data never receives a CSV candidate.

Prose is not put to a per-prompt vote. Anthropic's guide says XML tags help
"especially when your prompt mixes instructions, context, examples, and
variable inputs", names no format as better, and notes that a prompt's style
can shape the reply's. So a draft of one kind of content (every segment the
same role) goes out as plain text: your words with the approved edits, in their
original layout, with any notes in a small XML block after them. A draft that
mixes kinds goes out in its target's format, where the model name decides
(`claude-*` → Anthropic; `gpt-*`, `codex-*`, `o1`/`o3`/`o4` → OpenAI), with the
harness as the fallback:

| Target | Format | Source |
| --- | --- | --- |
| Anthropic (Claude Code) | XML tags per segment | Anthropic's [prompting guide](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices): wrapping each kind of content in its own tag "reduces misinterpretation". |
| OpenAI (Codex, GPT-6) | Markdown `##` sections, notes in an XML block | OpenAI's [prompt-engineering guide](https://developers.openai.com/api/docs/guides/prompt-engineering#message-formatting-with-markdown-and-xml) (GPT-6 examples): Markdown headers "mark distinct sections" and XML tags "delineate where one piece of content … begins and ends". Codex's GPT-6 [instructions](https://github.com/openai/codex/blob/15922a50aa5fae5fb8998ce6079a8d0099b539d0/codex-rs/models-manager/models.json#L249) are Markdown and its [injected context](https://github.com/openai/codex/blob/15922a50aa5fae5fb8998ce6079a8d0099b539d0/codex-rs/protocol/src/protocol.rs#L118-L137) is XML. |
| Unknown | XML tags | Both vendors document XML-delimited sections. |

OpenAI has no GPT-6-specific formatting guide; its GPT-6
[model guide](https://developers.openai.com/api/docs/guides/latest-model)
covers behavior, not prompt layout. Neither vendor documents TOON. Sections use
the names from Anthropic's guide (`instructions`, `constraints`, `questions`,
`context`, `input`, `output_format`, `examples`), which also match OpenAI's
prompt sections; neighbouring segments of one role share a section. Segments
keep the user's order and text verbatim, code blocks included:

```xml
<instructions>Audit the plugin.</instructions>
<constraints>
Keep the intention lossless.
Do not change the public API.
</constraints>
<standardizer_notes>
<earlier_context>[assistant] Two fixes remain: the Codex skill and the live test.</earlier_context>
<research>Inspect relevant local project files or documentation before answering.</research>
</standardizer_notes>
```

```markdown
## Instructions
Audit the plugin.

## Constraints
Keep the intention lossless.
Do not change the public API.

<standardizer_notes>
<research>Inspect relevant local project files or documentation before answering.</research>
</standardizer_notes>
```

If the target's format is unavailable because the text already contains a
closing tag it would use, the other prose format is used. Structured (non-prose)
input still gets JSON, TOON, CSV, or XML by Jev's choice, or its smallest
encoding when Jev is not asked.

You decide the format when the rule does not suit you:
`JEV_STANDARDIZER_FORMAT=plain|xml|markdown|json` sets a default, and the
review screen switches formats with one key. The response carries every
switchable rendering in `alternatives`, so switching costs no request.

Notes carry only what the harness cannot know: Jev's judgments (`answer_style`,
`research`) and quoted outside material (`earlier_context`), grouped under
`standardizer_notes` so the model can tell them from the user's words.

Filler removal is limited to politeness and framing: `please`, `kindly`,
`basically`, `I'd/I would like you to`, and `I want you to`. Words that can
carry intent — `just` (only), `if possible` (optional), `actually`
(correction), `simply`, `can/could/would you` (question versus command), and
`when you get a chance` (priority) — are never deletion candidates.

Pressing Alt+G again on an accepted payload restores the original draft first.
On acceptance the editor keeps `{payload, original}` for that session in
`$XDG_RUNTIME_DIR` (mode 0600, cleared at logout); a later draft that starts
with that payload is standardized from the original text plus whatever was
appended, instead of nesting the escaped payload.

Code owns every transformation. Optional local Hunspell supplies at most eight
close spelling alternatives; missing Hunspell simply disables typo candidates.
Code also proposes a few safe idiom reductions and question casing/punctuation
fixes. Jev chooses or rejects these in the existing single request, and the
preview shows applied edits. This is bounded editing, not arbitrary prose
generation or a guarantee that every grammatical error will be fixed. Research
is a flag for the downstream agent; Alt+G never fetches sources itself.
For structured data, auto encoding uses Jev's choice when it clears
`minConfidence` and the smallest encoding otherwise. The response
contains the exact downstream `text`, its `alternatives`, every decision, and
timing/token estimates.

The editor draws the review on a temporary alternate terminal screen, so
returning to Codex or Claude Code does not leave the host TUI mixed with the
prompt. While that screen is active, it enables terminal newline processing so
lines stay aligned even when a host TUI disables it, then restores the host's
exact terminal mode on exit. The screen is a header (format, target, and why:
"one kind of content", "mixed content", "your default", or "your choice"), the
prompt (40 lines at most on screen; the composer gets all of it), one Jev line,
and the keys. The Jev line collapses what applied to its answer (`✓ question ×2
· teh → the · concise answer`) and lists, with its confidence, only what Jev
answered below `minConfidence` that would have changed the prompt (`? s2 context
0.55 · s1|s2 split 0.62`), then the time and any backup request. When the bridge
or Jev fails (for example TypeSafe's 529 "high traffic"), the screen shows the
error and `r` retries; any other key keeps the draft. In WezTerm, `s` accepts and
then sends: a detached `wezterm cli send-text` presses Enter in the pane 0.4 s
after the editor exits, once the host has taken the text back. Prompt tags are cyan, Markdown
headings blue, and the notes block dim, in standard ANSI colors so the
terminal's palette applies; `NO_COLOR` or a non-terminal output disables them.
Every decision and probability remains in the API response and the bridge
journal.

Question-punctuation edits never turn a command into a question: a sentence
starting with "Do", "Does", or "Did" gains a "?" only when a subject follows
("Do we…"), so "Do not break the layout." keeps its period.

## Local installation

Build the long-running bridge and the small external-editor client:

```sh
cargo build -p jev-input-standardizer --release --features server \
  --bin jev-input-standardizer-server \
  --bin jev-input-standardizer-editor
install -m 755 target/release/jev-input-standardizer-server \
  "$HOME/.local/bin/jev-input-standardizer-server"
install -m 755 target/release/jev-input-standardizer-editor \
  "$HOME/.local/bin/jev-input-standardizer-editor"
install -m 755 scripts/jev-input-standardizer-codex \
  "$HOME/.local/bin/jev-input-standardizer-codex"
install -m 755 scripts/jev-input-standardizer-claude \
  "$HOME/.local/bin/jev-input-standardizer-claude"
```

Put `TYPESAFE_API_KEY=...` in the file the unit's `EnvironmentFile=` names. The
included `deploy/jev-input-standardizer.service` runs the bridge persistently on
`127.0.0.1:8788`.

```sh
systemctl --user link "$PWD/deploy/jev-input-standardizer.service"
systemctl --user daemon-reload
systemctl --user enable --now jev-input-standardizer.service
```

On this workstation, ordinary `codex` and `claude` shell launches and WezTerm's
agent launcher are configured to select the editor automatically. The explicit
launchers remain available as portable fallbacks:

```sh
jev-input-standardizer-codex
jev-input-standardizer-claude
```

The older `jev-codex`, `jev-claude`, and `jev-context-standardizer` CLI/service
names remain compatibility aliases; the obsolete plugin package and marketplace
entry are removed.
The launchers pass every command-line argument through to the underlying host.
For an unconfigured shell, the equivalent one-off setup is:

```sh
VISUAL=jev-input-standardizer-editor EDITOR=jev-input-standardizer-editor codex
VISUAL=jev-input-standardizer-editor EDITOR=jev-input-standardizer-editor claude
```

## Latency and memory controls

- Boundary, role, filler, edit, answer style, research, context, and
  structured-data encoding judgments use one native Jev fan-out request.
- Jev's latency is service-side noise: median about 0.55 s, 90th percentile
  about 1.5 s, warm or cold, and neither question count nor tokens move it much.
  A request still pending after `hedgeAfterMs` (700 ms) gets one identical
  backup and the first answer wins, so a slow Alt+G costs two Jev requests;
  the backup's deadline keeps the overall timeout. `hedgeAfterMs: 0` disables
  it.
- The bridge keeps its Jev connection warm (no pool idle timeout, TCP
  keepalive), saving a TCP and TLS handshake (about 0.5 s to the Oregon-hosted API) when presses
  are more than 90 s apart.
- The SDK speaks HTTP/2, so the hedged backup request shares the warm connection instead
  of opening a new one (about 0.5 s of handshakes to the API).
- Local spelling candidates are capped at 96 words and a 300 ms process
  deadline. No web request is made for research during `Alt+G`.
- The hotkey always asks Jev (`jevMinChars: 0`); API callers retain the
  deterministic sub-160-character fast path by default, except for a short
  explicit follow-up question that needs a split judgment, or sentences that
  already look like different kinds (a question, then an instruction).
- Jev retries are disabled. API callers default to a two-second deadline; the
  explicit hotkey allows three seconds before failing open.
- The editor client uses one direct loopback TCP connection. It does not launch
  `curl`, `jq`, a webview, or another editor.
- The confirmation prompt reads one key and restores the host terminal mode;
  it does not wait for a newline from Codex or Claude Code's raw terminal.
- The preview uses an alternate terminal screen and disappears when the editor
  exits; the accepted full payload remains visible in the host composer.
- The bridge reuses the SDK HTTP client and connection pool. There is no result
  cache or background worker pool. The editor reads a bounded transcript tail
  and keeps only the last accepted payload per session in `$XDG_RUNTIME_DIR`.
- Background adds Jev input tokens (up to about 6k for twelve full turns); the
  bridge caps it at 24,000 characters, newest items first, so hook items (sent
  last) are kept before older turns.
- Requests are capped at 128 KiB and context defaults to 64 KiB.
- The TOON crate is built without CLI, TUI, or tokenizer features.
- Failure and rejection leave the original draft untouched.

Peak transformation memory includes the typed input and one JSON plus one TOON
candidate so the final choice uses their actual sizes. The package targets the
official Rust TOON v3 encoder (`toon-format` 0.5); the newer v4.1 specification
is still a working draft.

### Measured release behavior

On 2026-09-23 from Singapore, a live three-boundary prompt completed in 714 ms
with one Jev request. After the request, the bridge used about 7.9 MiB RSS and
had peaked at about 8.9 MiB. Release binary sizes were about 709 KiB for the
editor and 6.0 MiB for the bridge. Jev latency varies; these are point-in-time
observations, not an SLA.

A live 25-character question with spelling and question edits took 503 ms,
one Jev request, and about 9.2 MiB service memory. It used 1,346 Jev input
tokens to turn an estimated 9-token prompt into a 15-token JSON payload.
This workflow improves review and structure, but does **not** save tokens on
short questions; use Alt+G selectively when the preview is worth its cost.

On 2026-09-24 (0.6.0), a live two-sentence Claude Code draft with the target
model and four conversation turns completed in 477 ms with one Jev request and
1,966 Jev input tokens; Jev chose XML with probability 0.93. The installed
bridge used about 8.0 MiB RSS after its first request. Release binaries were
about 940 KiB for the editor and 6.3 MiB for the bridge.

On 2026-09-24 (0.7.0), a live Claude Code draft "go with the first option."
with ten conversation turns completed in 711 ms with one Jev request. Jev put
0.72 on the assistant turn that listed the options and at most 0.10 on every
other turn, so only that turn was quoted; a self-contained draft quoted none.

## API

Set `TYPESAFE_API_KEY`, then provide JSON on stdin:

```sh
cargo run -p jev-input-standardizer --release <<'JSON'
{
  "context": "Please implement the parser.\n\nDo not change the public API.",
  "options": {"format": "auto"}
}
JSON
```

The loopback bridge exposes:

- `GET /health`
- `POST /standardize`

`GET /health` includes process-lifetime usage counters. The service also emits
one metadata-only journal line per attempt; it never logs prompt or payload
text. Follow it with:

```sh
journalctl --user -u jev-input-standardizer.service -f
```

Configuration:

- `JEV_STANDARDIZER_LISTEN`: bridge listen address;
- `JEV_STANDARDIZER_ENDPOINT`: loopback base URL used by the editor client;
- `JEV_STANDARDIZER_EDITOR_TIMEOUT_MS`: editor-side deadline, 4,000 ms by
  default and clamped between 100 and 30,000 ms;
- `JEV_STANDARDIZER_CONTEXT_CHARS`: conversation characters the editor sends,
  24,000 by default and at most, `0` to disable;
- `JEV_STANDARDIZER_CONTEXT_CMD`: optional context hook, described above;
- `JEV_STANDARDIZER_FORMAT`: default format, `auto` unless set to `plain`,
  `xml`, `markdown`, `json`, `toon`, or `csv`.

API callers pass the same context in `options`: `host` (a harness id such as
`claude` or `codex`), `targetModel`, `background` (`[{"source": ..., "text":
...}]`, oldest first), `hedgeAfterMs`, and `minConfidence` (0.8; it replaces
the old per-judgment thresholds). `format` accepts `auto`, `plain`,
`xml`, `markdown`, `json`, `toon`, or `csv`. The response explains the choice in
`encodingDecision.reason`, lists `alternatives`, and lists per-item
`contextDecisions` for outside background. The editor
sends and parses the library's own `StandardizeOptions` and
`StandardizeResult`, so the two sides cannot drift.

## Extending

- A harness is one row in `src/harness.rs` (`id`, display `name`, skill syntax,
  default vendor) plus, for conversation context, a transcript reader in the
  `READERS` table of `src/bin/editor/session.rs`, and a launcher that sets
  `JEV_STANDARDIZER_HOST`.
- A model vendor is one row in `src/target.rs` (`VENDORS`: model-name prefixes,
  prose format, and the sourced reason) plus a `Family` variant.
- Custom context needs no code: point `JEV_STANDARDIZER_CONTEXT_CMD` at any
  script.

Code map: `draft.rs` prepares the input once; `jev.rs` makes the one (hedged)
request built by `questions.rs`; `decisions.rs` applies the answers;
`context.rs` and `guidance.rs` add notes; `encoding.rs` chooses the format that
`render.rs` produces. The editor's `bridge.rs`, `session.rs`, `hook.rs`,
`memory.rs`, `terminal.rs`, `screen.rs`, and `paint.rs` each own one concern.

## Plugins

The package root is a Codex plugin and `claude/` is a Claude Code plugin. They
provide discovery and usage guidance. Native Codex and Claude keymaps bind
`Alt+G`; host-scoped environment selects the editor client. There are
deliberately no `UserPromptSubmit` hooks.

Install the Claude Code plugin from GitHub:

```sh
claude plugin marketplace add PhiDung-hub/jev-input-standardizer
claude plugin install jev-input-standardizer@jev-input-standardizer
```

## Verification

```sh
cargo fmt --check
cargo clippy -p jev-input-standardizer --all-targets --all-features -- -D warnings
cargo test -p jev-input-standardizer --all-targets --all-features
python3 ~/.codex/skills/.system/plugin-creator/scripts/validate_plugin.py .
python3 ~/.codex/skills/.system/skill-creator/scripts/quick_validate.py \
  skills/jev-input-standardizer
claude plugin validate --strict claude/
```

## Eval

`evals/cases.jsonl` holds 25 look-alike drafts, modelled on real Alt+G use, each with the format it
should come out in, whether notes are expected, and phrases that must survive.
The live run sends each one with the Alt+G editor's options and scores four
checks per case:
1. **Lossless:** every draft word is still there, unless an applied filler
   removal or edit explains it.
2. **Format:** the output uses the expected format.
3. **Notes:** notes appear only where expected.
4. **Keep:** the must-keep phrases survive.

```sh
cargo test -p jev-input-standardizer --test eval --release -- --ignored --nocapture
```

It fails when the total drops below `evals/baseline.json` and writes each output to
`evals/results/latest.json`, which is gitignored. After an intended change, rerun
it with `JEV_EVAL_BASELINE=update`. Runs can vary by a check, so the baseline
is the lowest total seen.
