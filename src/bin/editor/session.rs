use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use jev_input_standardizer::BackgroundItem;
use jev_input_standardizer::harness::Harness;
use serde_json::Value;

// ponytail: only the last 4 MiB is read, so turns older than that window are invisible.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;
const FIRST_LINE_BYTES: u64 = 1024 * 1024;
const MAX_TURNS: usize = 12;
const TURN_EDGE_CHARS: usize = 1_000;
const CODEX_DAYS: usize = 14;
const ANCESTOR_DEPTH: usize = 4;

pub(super) struct SessionContext {
    /// Transcript file stem; keys per-session editor state.
    pub id: Option<String>,
    pub model: Option<String>,
    pub turns: Vec<BackgroundItem>,
}

type Loaded = (Option<PathBuf>, Option<String>, Vec<BackgroundItem>);

type Reader = fn() -> Loaded;

/// Transcript readers by harness id. A new harness adds its reader here.
const READERS: &[(&str, Reader)] = &[("claude", load_claude), ("codex", load_codex)];

pub(super) fn load(harness: Option<&Harness>, budget_chars: usize) -> SessionContext {
    let reader = harness.and_then(|harness| {
        READERS
            .iter()
            .find(|(id, _)| *id == harness.id)
            .map(|(_, reader)| *reader)
    });
    let (path, model, turns) = reader.map_or((None, None, Vec::new()), |reader| reader());
    SessionContext {
        id: path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(|stem| sanitize(&stem.to_string_lossy())),
        model,
        turns: clip_turns(turns, budget_chars),
    }
}

fn load_claude() -> Loaded {
    let path = claude_transcript();
    let tail = path.as_deref().and_then(|path| read_tail(path, TAIL_BYTES));
    let (model, turns) = parse_claude(&tail.unwrap_or_default());
    let model = model.or_else(|| env::var("ANTHROPIC_MODEL").ok().filter(|m| !m.is_empty()));
    (path, model, turns)
}

fn load_codex() -> Loaded {
    let home = env_dir("CODEX_HOME", ".codex");
    let path = home.as_deref().and_then(codex_rollout);
    let tail = path.as_deref().and_then(|path| read_tail(path, TAIL_BYTES));
    let (model, turns) = parse_codex(&tail.unwrap_or_default());
    let model =
        model.or_else(|| config_model(&fs::read_to_string(home?.join("config.toml")).ok()?));
    (path, model, turns)
}

fn claude_transcript() -> Option<PathBuf> {
    let config = env_dir("CLAUDE_CONFIG_DIR", ".claude")?;
    claude_by_id(&config).or_else(|| claude_by_cwd(&config.join("projects")))
}

fn claude_by_id(config: &Path) -> Option<PathBuf> {
    let id = env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .or_else(|| ancestor_session(config))?;
    let file = format!("{}.jsonl", sanitize(&id)?);
    children(&config.join("projects"))
        .into_iter()
        .map(|project| project.join(&file))
        .find(|path| path.is_file())
}

/// Claude Code keeps `sessions/<pid>.json` but does not export its session ID to
/// the editor, so walk up from the editor to the owning Claude process.
#[cfg(unix)]
fn ancestor_session(config: &Path) -> Option<String> {
    let sessions = config.join("sessions");
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..ANCESTOR_DEPTH {
        if let Ok(text) = fs::read_to_string(sessions.join(format!("{pid}.json"))) {
            let entry: Value = serde_json::from_str(&text).ok()?;
            return entry["sessionId"].as_str().map(str::to_owned);
        }
        pid = parent_of(pid)?;
    }
    None
}

#[cfg(not(unix))]
fn ancestor_session(_config: &Path) -> Option<String> {
    None
}

#[cfg(unix)]
fn parent_of(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn claude_by_cwd(projects: &Path) -> Option<PathBuf> {
    let project = projects.join(claude_project_key(&env::current_dir().ok()?));
    newest_first(children(&project)).find(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
}

fn claude_project_key(cwd: &Path) -> String {
    let key = cwd.to_string_lossy();
    key.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn codex_rollout(home: &Path) -> Option<PathBuf> {
    let mut days: Vec<PathBuf> = children(&home.join("sessions"))
        .iter()
        .flat_map(|year| children(year))
        .flat_map(|month| children(&month))
        .collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    days.truncate(CODEX_DAYS);
    rollout_by_id(&days).or_else(|| rollout_by_cwd(&days))
}

fn rollout_by_id(days: &[PathBuf]) -> Option<PathBuf> {
    let suffix = format!("-{}.jsonl", sanitize(&env::var("CODEX_THREAD_ID").ok()?)?);
    days.iter()
        .flat_map(|day| children(day))
        .find(|path| path.to_string_lossy().ends_with(&suffix))
}

fn rollout_by_cwd(days: &[PathBuf]) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    newest_first(days.iter().flat_map(|day| children(day)).collect())
        .find(|path| session_cwd(path).as_deref() == Some(cwd.as_path()))
}

fn newest_first(mut paths: Vec<PathBuf>) -> impl Iterator<Item = PathBuf> {
    paths.sort_by_cached_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok());
    paths.into_iter().rev()
}

fn session_cwd(path: &Path) -> Option<PathBuf> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?.take(FIRST_LINE_BYTES))
        .read_line(&mut line)
        .ok()?;
    let meta: Value = serde_json::from_str(&line).ok()?;
    let cwd = meta["payload"]["cwd"]
        .as_str()
        .filter(|_| meta["type"] == "session_meta");
    cwd.map(PathBuf::from)
}

fn children(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn env_dir(var: &str, home_child: &str) -> Option<PathBuf> {
    match env::var_os(var) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => env::var_os("HOME").map(|home| Path::new(&home).join(home_child)),
    }
}

fn sanitize(id: &str) -> Option<String> {
    let id = id.trim();
    let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| id.to_owned())
}

fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes).ok()?;
    let skip = match bytes.iter().position(|&b| b == b'\n') {
        Some(newline) if start > 0 => newline + 1,
        _ => 0,
    };
    Some(String::from_utf8_lossy(&bytes[skip..]).into_owned())
}

fn parse_claude(tail: &str) -> (Option<String>, Vec<BackgroundItem>) {
    let candidate = |line: &str| {
        let user = line.contains(r#""type":"user""#) && !line.contains(r#""type":"tool_result""#);
        user || (line.contains(r#""type":"assistant""#) && line.contains(r#""type":"text""#))
    };
    let mut model = None;
    let mut turns = Vec::new();
    for entry in json_lines(tail, candidate) {
        let message = &entry["message"];
        let noise = entry["isSidechain"] == true || entry["isMeta"] == true;
        if noise || message["model"] == "<synthetic>" {
            continue;
        }
        if entry["type"] == "assistant" {
            model = message["model"].as_str().map(str::to_owned).or(model);
        }
        push_turn(&mut turns, entry["type"].as_str(), &message["content"]);
    }
    (model, turns)
}

fn parse_codex(tail: &str) -> (Option<String>, Vec<BackgroundItem>) {
    let candidate = |line: &str| {
        let item =
            line.contains(r#""type":"response_item""#) && line.contains(r#""type":"message""#);
        item || line.contains(r#""type":"turn_context""#)
    };
    let mut model = None;
    let mut turns = Vec::new();
    for entry in json_lines(tail, candidate) {
        let payload = &entry["payload"];
        if entry["type"] == "turn_context" {
            model = payload["model"].as_str().map(str::to_owned).or(model);
        } else if entry["type"] == "response_item" && payload["type"] == "message" {
            push_turn(&mut turns, payload["role"].as_str(), &payload["content"]);
        }
    }
    (model, turns)
}

fn json_lines(tail: &str, candidate: impl Fn(&str) -> bool) -> impl Iterator<Item = Value> {
    tail.lines()
        .filter(move |line| candidate(line))
        .filter_map(|line| serde_json::from_str(line).ok())
}

fn push_turn(turns: &mut Vec<BackgroundItem>, role: Option<&str>, content: &Value) {
    let (role, keep): (&'static str, fn(&str) -> bool) = match role {
        Some("user") => ("user", is_prompt),
        Some("assistant") => ("assistant", |_| true),
        _ => return,
    };
    let blocks = content.as_array().map_or(&[][..], Vec::as_slice);
    let texts = content
        .as_str()
        .into_iter()
        .chain(blocks.iter().filter_map(|block| block["text"].as_str()));
    let kept: Vec<&str> = texts
        .map(str::trim)
        .filter(|text| !text.is_empty() && keep(text))
        .collect();
    if kept.is_empty() {
        return;
    }
    let text = kept.join("\n");
    match turns.last_mut() {
        Some(last) if last.source == role => last.text = format!("{}\n\n{text}", last.text),
        _ => turns.push(BackgroundItem {
            source: role.to_owned(),
            text,
        }),
    }
}

fn is_prompt(text: &str) -> bool {
    !text.starts_with('<') && !text.starts_with("# AGENTS.md")
}

fn clip_turns(mut turns: Vec<BackgroundItem>, budget_chars: usize) -> Vec<BackgroundItem> {
    turns.drain(..turns.len().saturating_sub(MAX_TURNS));
    for turn in &mut turns {
        turn.text = clip(&turn.text);
    }
    let mut total = 0;
    let keep = turns
        .iter()
        .rev()
        .take_while(|turn| {
            total += turn.text.chars().count();
            total <= budget_chars
        })
        .count();
    turns.drain(..turns.len() - keep);
    turns
}

/// Keeps both ends of a long text, which carry the ask and the conclusion.
pub(super) fn clip(text: &str) -> String {
    let count = text.chars().count();
    if count <= 2 * TURN_EDGE_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(TURN_EDGE_CHARS).collect();
    let tail: String = text.chars().skip(count - TURN_EDGE_CHARS).collect();
    format!("{head} … {tail}")
}

fn config_model(config: &str) -> Option<String> {
    let value = config
        .lines()
        .take_while(|line| !line.trim_start().starts_with('['))
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "model").then_some(value)
        })?;
    let model = value
        .trim()
        .strip_prefix(['"', '\''])?
        .split(['"', '\''])
        .next()?;
    (!model.is_empty()).then(|| model.to_owned())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const CLAUDE_FIXTURE: &str = r#"{"type":"file-history-snapshot","snapshot":{}}
{"type":"user","isMeta":true,"isSidechain":false,"message":{"role":"user","content":"<local-command-caveat>Caveat</local-command-caveat>"}}
{"type":"user","isSidechain":false,"message":{"role":"user","content":"<command-name>/effort</command-name>"}}
{"type":"user","isSidechain":false,"promptSource":"typed","message":{"role":"user","content":"fix the parser"}}
{"type":"assistant","isSidechain":false,"message":{"model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"hmm"}]}}
{"type":"assistant","isSidechain":false,"message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Looking at it."}]}}
{"type":"assistant","isSidechain":false,"message":{"model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]}}
{"type":"user","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"big output"}]}}
{"type":"assistant","isSidechain":false,"message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"Fixed."},{"type":"text","text":"  "}]}}
{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5","content":[{"type":"text","text":"sidechain"}]}}
{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent prompt"}}
{"type":"assistant","isSidechain":false,"message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}}
{"type":"attachment","attachment":{"type":"user","content":"ignored"}}
{"type":"user","isSidechain":false,"message":{"role":"user","content":[{"type":"text","text":"<system-reminder>x</system-reminder>"},{"type":"text","text":"now ship it"}]}}
{"type":"assistant","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"partial line"#;

    const CODEX_FIXTURE: &str = r##"{"type":"session_meta","payload":{"id":"t","cwd":"/work"}}
{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"sandbox rules"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\n\n<INSTRUCTIONS>"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/work</cwd>"}]}}
{"type":"turn_context","payload":{"cwd":"/work","model":"gpt-5-old"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<image name=[Image #1]>"},{"type":"input_image","image_url":"x"},{"type":"input_text","text":"what is this?"}]}}
{"type":"response_item","payload":{"type":"reasoning","summary":[]}}
{"type":"response_item","payload":{"type":"function_call_output","output":"{\"type\":\"message\"}"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"A cat."}]}}
{"type":"turn_context","payload":{"cwd":"/work","model":"gpt-6-sol"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"thanks"}]}}
"##;

    fn turn(role: &str, text: &str) -> BackgroundItem {
        BackgroundItem {
            source: role.to_owned(),
            text: text.to_owned(),
        }
    }

    fn pairs(turns: &[BackgroundItem]) -> Vec<(&str, &str)> {
        turns
            .iter()
            .map(|turn| (turn.source.as_str(), turn.text.as_str()))
            .collect()
    }

    #[test]
    fn parse_claude_keeps_typed_prompts_and_assistant_text() {
        let (model, turns) = parse_claude(CLAUDE_FIXTURE);
        assert_eq!(model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(
            pairs(&turns),
            [
                ("user", "fix the parser"),
                ("assistant", "Looking at it.\n\nFixed."),
                ("user", "now ship it"),
            ]
        );
    }

    #[test]
    fn parse_claude_without_assistant_has_no_model() {
        let (model, turns) = parse_claude(r#"{"type":"user","message":{"content":"hi"}}"#);
        assert_eq!(model, None);
        assert_eq!(pairs(&turns), [("user", "hi")]);
    }

    #[test]
    fn parse_codex_reads_model_and_skips_harness_text() {
        let (model, turns) = parse_codex(CODEX_FIXTURE);
        assert_eq!(model.as_deref(), Some("gpt-6-sol"));
        assert_eq!(
            pairs(&turns),
            [
                ("user", "what is this?"),
                ("assistant", "A cat."),
                ("user", "thanks")
            ]
        );
    }

    #[test]
    fn clip_turns_clips_each_turn_on_char_boundaries() {
        let text = format!(
            "{}{}",
            "é".repeat(TURN_EDGE_CHARS + 100),
            "漢".repeat(TURN_EDGE_CHARS + 100)
        );
        let turns = clip_turns(vec![turn("user", &text)], 100_000);
        let expected = format!(
            "{} … {}",
            "é".repeat(TURN_EDGE_CHARS),
            "漢".repeat(TURN_EDGE_CHARS)
        );
        assert_eq!(turns[0].text, expected);
        assert_eq!(turns[0].text.chars().count(), 2 * TURN_EDGE_CHARS + 3);
    }

    #[test]
    fn clip_turns_keeps_the_newest_then_drops_oldest_over_budget() {
        let turns: Vec<BackgroundItem> = (0..MAX_TURNS + 2)
            .map(|i| turn("user", &format!("turn{i:02}")))
            .collect();
        let all = clip_turns(turns, 10_000);
        assert_eq!(all.len(), MAX_TURNS);
        assert_eq!(all[0].text, "turn02");
        let budgeted = clip_turns(all, 12);
        assert_eq!(pairs(&budgeted), [("user", "turn12"), ("user", "turn13")]);
    }

    #[test]
    fn clip_turns_zero_budget_is_empty() {
        assert!(clip_turns(vec![turn("user", "hi")], 0).is_empty());
    }

    #[test]
    fn read_tail_drops_partial_first_line() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path =
            env::temp_dir().join(format!("jev-session-{}-{nanos}.jsonl", std::process::id()));
        File::create(&path)
            .and_then(|mut file| file.write_all("first line\nsecond ✓\nthird\n".as_bytes()))
            .expect("write temp file");
        let tail = read_tail(&path, 16);
        let whole = read_tail(&path, 1024);
        fs::remove_file(&path).expect("remove temp file");
        assert_eq!(tail.as_deref(), Some("third\n"));
        assert_eq!(whole.as_deref(), Some("first line\nsecond ✓\nthird\n"));
        assert_eq!(read_tail(&path, 16), None);
    }

    #[test]
    fn config_model_reads_top_level_model_only() {
        let config = "model_reasoning_effort = \"xhigh\"\nmodel = \"gpt-6-sol\" # pinned\n[profiles.x]\nmodel = \"other\"\n";
        assert_eq!(config_model(config).as_deref(), Some("gpt-6-sol"));
        assert_eq!(config_model("[section]\nmodel = \"other\"\n"), None);
        assert_eq!(config_model("model = ''\n"), None);
    }

    #[test]
    fn claude_project_key_matches_claude_code_dir_names() {
        let key = claude_project_key(Path::new("/home/user/.config/nvim"));
        assert_eq!(key, "-home-user--config-nvim");
    }

    #[test]
    fn sanitize_rejects_path_characters() {
        assert_eq!(
            sanitize(" a1684e73-5695 ").as_deref(),
            Some("a1684e73-5695")
        );
        assert_eq!(sanitize("../etc"), None);
        assert_eq!(sanitize(""), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ancestor_session_reads_the_parent_process_entry() {
        let config = env::temp_dir().join(format!("jev-session-config-{}", std::process::id()));
        let sessions = config.join("sessions");
        fs::create_dir_all(&sessions).expect("create sessions dir");
        let parent = std::os::unix::process::parent_id();
        fs::write(
            sessions.join(format!("{parent}.json")),
            r#"{"pid":1,"sessionId":"abc-123"}"#,
        )
        .expect("write session entry");
        let found = ancestor_session(&config);
        fs::remove_dir_all(&config).expect("remove temp config");

        assert_eq!(found.as_deref(), Some("abc-123"));
        assert_eq!(parent_of(std::process::id()), Some(parent));
    }

    #[test]
    fn load_unknown_host_is_empty() {
        for harness in [None, jev_input_standardizer::harness::find("vim")] {
            let context = load(harness, 6_000);
            assert!(context.model.is_none() && context.turns.is_empty());
        }
    }
}
