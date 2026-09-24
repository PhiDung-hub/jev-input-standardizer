//! Custom background for Jev. `JEV_STANDARDIZER_CONTEXT_CMD` runs through `sh -c` in
//! the session's directory with the draft on stdin and `JEV_STANDARDIZER_TARGET_MODEL`
//! set, so a notes, AGENTS.md, or retrieval script can offer material. Each stdout line
//! `{"source": "...", "text": "..."}` is one item; any other output is one "custom" item.

use std::io::Write;
use std::process::{Command, Stdio};

use jev_input_standardizer::BackgroundItem;
use serde_json::Value;

use super::session::clip;

// ponytail: a slow hook delays every Alt+G by up to this deadline (plus a 1 s kill
// grace for hooks that ignore SIGTERM); cache inside the hook if needed.
const DEADLINE: &str = "2s";

pub(super) fn run(command: &str, draft: &str, model: Option<&str>) -> Vec<BackgroundItem> {
    let spawned = Command::new("timeout")
        .args(["-k", "1s", DEADLINE, "sh", "-c", command])
        .env("JEV_STANDARDIZER_TARGET_MODEL", model.unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return Vec::new();
    };
    // Feed stdin on its own thread while stdout drains, or a hook that echoes a draft
    // larger than the pipe buffer would deadlock against us.
    let feeder = child.stdin.take().map(|mut stdin| {
        let draft = draft.to_owned();
        std::thread::spawn(move || stdin.write_all(draft.as_bytes()))
    });
    let output = child.wait_with_output();
    if let Some(feeder) = feeder {
        let _ = feeder.join();
    }
    match output {
        Ok(output) if output.status.success() => parse(&String::from_utf8_lossy(&output.stdout)),
        _ => Vec::new(),
    }
}

fn parse(output: &str) -> Vec<BackgroundItem> {
    let items: Vec<_> = output.lines().filter_map(json_item).collect();
    if !items.is_empty() {
        return items;
    }
    let text = output.trim();
    if text.is_empty() {
        return Vec::new();
    }
    vec![BackgroundItem {
        source: "custom".to_owned(),
        text: clip(text),
    }]
}

fn json_item(line: &str) -> Option<BackgroundItem> {
    let value: Value = serde_json::from_str(line).ok()?;
    let text = value["text"].as_str()?.trim();
    let source = value["source"].as_str().unwrap_or("custom");
    (!text.is_empty()).then(|| BackgroundItem {
        source: source.to_owned(),
        text: clip(text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_lines_become_items_and_plain_text_one_custom_item() {
        let items =
            parse("{\"source\":\"notes\",\"text\":\"Ship on Fridays.\"}\n{\"text\":\"x\"}\n");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source, "notes");
        assert_eq!(items[1].source, "custom");
        let plain = parse("Project uses pnpm.\n");
        assert_eq!(plain[0].text, "Project uses pnpm.");
        assert!(parse("  \n").is_empty());
    }

    #[test]
    fn runs_the_hook_with_the_draft_on_stdin() {
        let items = run(
            "printf '{\"source\":\"echo\",\"text\":\"%s\"}' \"$(cat) for $JEV_STANDARDIZER_TARGET_MODEL\"",
            "fix it",
            Some("gpt-6-sol"),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "fix it for gpt-6-sol");
        assert!(run("exit 3", "draft", None).is_empty());
    }

    #[test]
    fn a_hook_echoing_a_large_draft_does_not_deadlock() {
        let started = std::time::Instant::now();
        let items = run("cat", &"x".repeat(200_000), None);
        assert_eq!(items.len(), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
