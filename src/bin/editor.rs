//! Explicit external-editor bridge for coding harnesses (Claude Code, Codex, …).

use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use jev_input_standardizer::harness::{self, Harness, SkillSyntax};
use jev_input_standardizer::{
    EnhancementOptions, FormatPreference, StandardizeOptions, StandardizeResult,
};

#[path = "editor/bridge.rs"]
mod bridge;
#[path = "editor/hook.rs"]
mod hook;
#[path = "editor/memory.rs"]
mod memory;
#[path = "editor/paint.rs"]
mod paint;
#[path = "editor/screen.rs"]
mod screen;
#[path = "editor/session.rs"]
mod session;
#[path = "editor/terminal.rs"]
mod terminal;

use paint::Paint;

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8788";
const DEFAULT_TIMEOUT_MS: u64 = 4_000;
const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
const DEFAULT_CONTEXT_CHARS: usize = 24_000;
/// Long enough for the host to take the draft back before Enter arrives.
const SEND_DELAY: &str = "0.4";
const MAX_CONTEXT_CHARS: usize = 24_000;

#[derive(Debug, thiserror::Error)]
enum EditorError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid bridge endpoint: {0}")]
    Endpoint(String),
    #[error("invalid bridge response: {0}")]
    Protocol(String),
    #[error("bridge returned HTTP {status}: {message}")]
    Bridge { status: u16, message: String },
}

fn main() {
    if let Err(error) = run() {
        let paint = Paint::detect(io::stderr().is_terminal());
        let message = format!("Jev standardization failed; draft left unchanged: {error}");
        eprintln!("{}", paint.red(&message));
    }
}

fn run() -> Result<(), EditorError> {
    let path = editor_path()?;
    let draft = fs::read_to_string(&path)?;
    let harness = env::var("JEV_STANDARDIZER_HOST")
        .ok()
        .and_then(|value| harness::find(&value));
    if let Some(reason) = skip_reason(&draft, harness) {
        eprintln!("Jev standardization skipped: {reason} Original draft kept.");
        return Ok(());
    }
    let session = session::load(harness, context_chars());
    let memory = memory::path(session.id.as_deref());
    let restored = memory::restore(&draft, memory.as_deref());

    let _screen = terminal::AlternateScreen::enter()?;
    if restored.is_some() {
        eprintln!(
            "Draft starts with the last accepted payload; standardizing its original text instead."
        );
    }
    let draft = restored.unwrap_or(draft);
    eprintln!(
        "Asking Jev to structure this draft for {}…",
        session.model.as_deref().unwrap_or("the target model")
    );
    let options = request_options(harness, session, &draft);
    let endpoint = bridge::parse_endpoint(
        &env::var("JEV_STANDARDIZER_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned()),
    )?;
    let attempt = || bridge::standardize(&endpoint, &draft, &options, editor_timeout());
    let Some(response) = with_retry(attempt, |error| terminal::ask_retry(&error.to_string()))?
    else {
        eprintln!("Jev unavailable; original draft kept.");
        return Ok(());
    };
    let can_send = env::var_os("WEZTERM_PANE").is_some_and(|pane| !pane.is_empty());
    let choice = review(&response, can_send)?;
    finish(&path, choice.as_ref(), &draft, memory.as_deref(), harness)
}

/// What the user accepted, and whether to send it right away.
struct Choice {
    text: String,
    send: bool,
}

/// Tries again for as long as the user asks; `None` once they give up.
fn with_retry<T>(
    mut attempt: impl FnMut() -> Result<T, EditorError>,
    mut ask: impl FnMut(&EditorError) -> io::Result<bool>,
) -> Result<Option<T>, EditorError> {
    loop {
        match attempt() {
            Ok(value) => return Ok(Some(value)),
            Err(error) if ask(&error)? => eprintln!("Asking Jev again…"),
            Err(_) => return Ok(None),
        }
    }
}

/// Shows the prompt and reads keys: a format key redraws it in that format (no new
/// request), `y` accepts what is shown, and `n`, Enter, or Esc cancels.
fn review(response: &StandardizeResult, can_send: bool) -> Result<Option<Choice>, EditorError> {
    let paint = Paint::detect(io::stdout().is_terminal());
    let _keys = terminal::Keys::open()?;
    let mut shown = response.encoding;
    loop {
        let mut output = io::stdout().lock();
        write!(output, "{CLEAR_SCREEN}")?;
        screen::draw(&mut output, response, shown, paint, can_send)?;
        output.flush()?;
        drop(output);
        let action = screen::action(terminal::read_key()?, response, can_send);
        let send = matches!(action, screen::Action::Send);
        match action {
            screen::Action::Accept | screen::Action::Send => {
                println!();
                let text = screen::text_of(response, shown).to_owned();
                return Ok(Some(Choice { text, send }));
            }
            screen::Action::Cancel => {
                println!();
                return Ok(None);
            }
            screen::Action::Show(encoding) => shown = encoding,
            screen::Action::Ignore => {}
        }
    }
}

fn skip_reason(draft: &str, harness: Option<&Harness>) -> Option<String> {
    if draft.trim().is_empty() {
        return Some("draft is empty.".to_owned());
    }
    let harness = harness.filter(|harness| harness.skills == SkillSyntax::Slash)?;
    starts_with_slash_command(draft).then(|| {
        format!(
            "{} slash commands must stay at the start of the draft.",
            harness.name
        )
    })
}

fn starts_with_slash_command(draft: &str) -> bool {
    let Some(command) = draft.strip_prefix('/') else {
        return false;
    };
    let token = command.split_whitespace().next().unwrap_or_default();
    token.starts_with(|character: char| character.is_ascii_lowercase())
        && token.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
}

/// The library's own options: harness, target model, and background from the
/// transcript plus any context hook.
fn request_options(
    harness: Option<&Harness>,
    session: session::SessionContext,
    draft: &str,
) -> StandardizeOptions {
    let mut background = session.turns;
    if let Ok(command) = env::var("JEV_STANDARDIZER_CONTEXT_CMD") {
        background.extend(hook::run(&command, draft, session.model.as_deref()));
    }
    StandardizeOptions {
        host: harness.map(|harness| harness.id.to_owned()),
        target_model: session.model,
        background,
        enhancements: EnhancementOptions {
            correct_typos: true,
            rephrase: true,
            response_guidance: true,
        },
        format: default_format(),
        jev_min_chars: 0,
        request_timeout_ms: 3_000,
        ..StandardizeOptions::default()
    }
}

fn finish(
    path: &Path,
    choice: Option<&Choice>,
    draft: &str,
    memory: Option<&Path>,
    harness: Option<&Harness>,
) -> Result<(), EditorError> {
    let paint = Paint::detect(io::stderr().is_terminal());
    let Some(choice) = choice else {
        eprintln!("{}", paint.dim("Rejected. Original draft kept."));
        return Ok(());
    };
    fs::write(path, &choice.text)?;
    if let Some(memory) = memory {
        let _ = memory::remember(memory, &choice.text, draft);
    }
    let host = harness.map_or("the composer", |harness| harness.name);
    let message = if choice.send && press_enter_later() {
        format!("Accepted. Sending it in {host}.")
    } else {
        format!("Accepted. Returning to {host}; press Enter to send.")
    };
    eprintln!("{}", paint.green(&message));
    Ok(())
}

/// Presses Enter in this `WezTerm` pane just after the host takes the draft back (it reads
/// the file when the editor exits), so accepting also sends. Detached so it outlives us.
fn press_enter_later() -> bool {
    let Some(pane) = env::var_os("WEZTERM_PANE") else {
        return false;
    };
    Command::new("setsid")
        .args([
            "-f",
            "sh",
            "-c",
            "sleep \"$1\"; wezterm cli send-text --no-paste --pane-id \"$0\" \"$(printf '\\r')\"",
        ])
        .arg(pane)
        .arg(SEND_DELAY)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

/// `JEV_STANDARDIZER_FORMAT`: auto (default), plain, xml, markdown, json, toon, or csv.
fn default_format() -> FormatPreference {
    env::var("JEV_STANDARDIZER_FORMAT")
        .ok()
        .and_then(|value| serde_json::from_value(value.trim().to_ascii_lowercase().into()).ok())
        .unwrap_or_default()
}

fn context_chars() -> usize {
    env::var("JEV_STANDARDIZER_CONTEXT_CHARS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_CONTEXT_CHARS)
        .min(MAX_CONTEXT_CHARS)
}

fn editor_path() -> Result<PathBuf, EditorError> {
    let mut args = env::args_os().skip(1);
    let path = args
        .next()
        .ok_or_else(|| EditorError::Protocol("expected the composer draft path".to_owned()))?;
    if args.next().is_some() {
        return Err(EditorError::Protocol(
            "expected exactly one composer draft path".to_owned(),
        ));
    }
    Ok(path.into())
}

fn editor_timeout() -> Duration {
    let milliseconds = env::var("JEV_STANDARDIZER_EDITOR_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(100, 30_000);
    Duration::from_millis(milliseconds)
}

#[cfg(test)]
mod tests {
    use jev_input_standardizer::{
        ContextDecision, DecisionSource, EditDecision, GuidanceDecision, RoleDecision, SegmentRole,
        SegmentationDecision, standardize,
    };
    use typesafe_ai::{Client, Json};

    use super::*;

    #[test]
    fn slash_commands_stay_at_the_start_for_slash_harnesses_only() {
        let claude = harness::find("claude");
        assert!(skip_reason("/deploy production", claude).is_some());
        assert!(skip_reason("/review", claude).is_some());
        assert!(skip_reason("/home/user/notes", claude).is_none());
        assert!(skip_reason("Please /deploy production", claude).is_none());
        assert!(skip_reason("/deploy production", harness::find("codex")).is_none());
        assert!(skip_reason("  \n", None).is_some());
    }

    /// A real library result (fast path, no network) with every decision kind filled in.
    fn sample_result() -> StandardizeResult {
        let client = Client::builder()
            .api_key("test")
            .base_url("http://127.0.0.1:9")
            .build()
            .unwrap();
        let options = StandardizeOptions {
            host: Some("claude".to_owned()),
            target_model: Some("claude-opus-5-5".to_owned()),
            ..StandardizeOptions::default()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut result = runtime
            .block_on(standardize(
                &client,
                &Json::from("Test the parser."),
                &options,
            ))
            .unwrap();
        result.context_decisions = vec![ContextDecision {
            id: "b13".to_owned(),
            source: "notes".to_owned(),
            include_probability: Some(0.92),
            included: true,
        }];
        result.segmentation_decisions = vec![SegmentationDecision {
            id: "b1".to_owned(),
            before: "s1".to_owned(),
            after: "s2".to_owned(),
            split_probability: Some(0.9),
            split: true,
            source: DecisionSource::Jev,
        }];
        result.edit_decisions = vec![EditDecision {
            id: "e1".to_owned(),
            kind: "typo".to_owned(),
            path: "segments[0].text".to_owned(),
            original: "teh".to_owned(),
            replacement: Some("the".to_owned()),
            applied: true,
            confidence: Some(0.95),
        }];
        result.answer_style_decision = Some(GuidanceDecision {
            choice: "concise".to_owned(),
            confidence: Some(0.9),
            applied: true,
        });
        result.stats.jev_called = true;
        result.stats.jev_requests = 1;
        result.stats.background_items = 4;
        result
    }

    #[test]
    fn a_library_result_round_trips_and_draws_a_lean_screen() {
        let body = serde_json::to_vec(&sample_result()).unwrap();
        let mut response =
            format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len()).into_bytes();
        response.extend_from_slice(&body);
        let parsed = bridge::parse_response(&response).unwrap();
        assert_eq!(parsed.text, "Test the parser.");

        let mut plain = Vec::new();
        screen::draw(&mut plain, &parsed, parsed.encoding, Paint::PLAIN, false).unwrap();
        let plain = String::from_utf8(plain).unwrap();
        assert_eq!(plain.lines().count(), 4);
        assert!(plain.starts_with(
            "── plain for claude-opus-5-5 · one kind of content ──\nTest the parser.\n"
        ));
        assert!(plain.contains("── jev ✓ teh → the · concise answer · notes context · 0.0 s ──"));
        assert!(plain.ends_with("Accept? y/n · p plain · x XML · m Markdown · j JSON "));

        let mut colored = Vec::new();
        screen::draw(&mut colored, &parsed, parsed.encoding, paint::COLOR, false).unwrap();
        assert_eq!(paint::strip(&String::from_utf8(colored).unwrap()), plain);
    }

    #[test]
    fn only_uncertain_judgments_that_would_change_the_prompt_show() {
        let mut result = sample_result();
        let role = |id: &str, role, answer| RoleDecision {
            id: id.to_owned(),
            role,
            answer: Some(answer),
            confidence: Some(0.55),
            applied: false,
        };
        result.role_decisions = vec![
            role("s1", SegmentRole::Task, SegmentRole::Task),
            role("s2", SegmentRole::Context, SegmentRole::Question),
        ];
        let split = &mut result.segmentation_decisions[0];
        split.split_probability = Some(0.62);
        split.split = false;
        split.source = DecisionSource::Heuristic;
        result.research_decision = Some(GuidanceDecision {
            choice: "web".to_owned(),
            confidence: Some(0.75),
            applied: false,
        });
        let drawn = draw(&result, result.encoding, false);
        assert!(
            drawn.starts_with("── plain for claude-opus-5-5 · mixed content, kept as typed ──")
        );
        assert!(drawn.contains(
            "│ ? s1 task 0.55 · s2 question 0.55 · s1|s2 split 0.62 · web research 0.75 · 0.0 s"
        ));
        let mut lone = result.clone();
        lone.role_decisions.truncate(1);
        assert!(!draw(&lone, lone.encoding, false).contains("s1 task"));

        result.segmentation_decisions[0].split = true;
        assert!(!draw(&result, result.encoding, false).contains("split 0.62"));
        result.edit_decisions[0].original = String::new();
        result.edit_decisions[0].replacement = Some("?".to_owned());
        assert!(draw(&result, result.encoding, false).contains("── jev ✓ +? · concise answer"));
    }

    fn draw(
        result: &StandardizeResult,
        shown: jev_input_standardizer::Encoding,
        can_send: bool,
    ) -> String {
        let mut drawn = Vec::new();
        screen::draw(&mut drawn, result, shown, Paint::PLAIN, can_send).unwrap();
        String::from_utf8(drawn).unwrap()
    }

    #[test]
    fn format_keys_switch_the_shown_prompt_and_y_or_s_accepts_it() {
        let result = sample_result();
        let screen::Action::Show(encoding) = screen::action(b'x', &result, false) else {
            panic!("x should switch to XML");
        };
        assert_eq!(
            screen::text_of(&result, encoding),
            "<instructions>Test the parser.</instructions>"
        );
        assert!(
            draw(&result, encoding, false)
                .starts_with("── XML for claude-opus-5-5 · your choice ──")
        );
        assert!(matches!(
            screen::action(b'y', &result, false),
            screen::Action::Accept
        ));
        assert!(matches!(
            screen::action(27, &result, false),
            screen::Action::Cancel
        ));
        assert!(matches!(
            screen::action(b'q', &result, false),
            screen::Action::Ignore
        ));
        assert!(matches!(
            screen::action(b's', &result, false),
            screen::Action::Ignore
        ));
        assert!(matches!(
            screen::action(b's', &result, true),
            screen::Action::Send
        ));
        assert!(draw(&result, encoding, true).contains("Accept? y/n · s send · p plain"));
        assert!(!draw(&result, encoding, false).contains("s send"));
    }

    #[test]
    fn a_failed_request_is_retried_only_while_the_user_asks() {
        let mut failures = 2;
        let attempt = || {
            if failures == 0 {
                return Ok("prompt");
            }
            failures -= 1;
            Err(EditorError::Protocol("529 high traffic".to_owned()))
        };
        assert_eq!(with_retry(attempt, |_| Ok(true)).unwrap(), Some("prompt"));

        let mut asked = 0;
        let declined = with_retry(
            || Err::<&str, _>(EditorError::Protocol("down".to_owned())),
            |error| {
                asked += 1;
                assert_eq!(error.to_string(), "invalid bridge response: down");
                Ok(false)
            },
        );
        assert_eq!(declined.unwrap(), None);
        assert_eq!(asked, 1);
    }
}
