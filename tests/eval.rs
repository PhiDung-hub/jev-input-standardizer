//! Golden-case eval: Alt+G drafts through the live standardizer, each scored
//! on five checks worth six points (a wrong tag costs two, lost structure one).
//! Run it after any change that could move the output:
//!
//! `cargo test -p jev-input-standardizer --test eval --release -- --ignored --nocapture`
//!
//! It fails when the total score drops below `evals/baseline.json`; set
//! `JEV_EVAL_BASELINE=update` to record a new baseline after an intended change.

use std::collections::{BTreeMap, HashMap};
use std::{env, fs};

use jev_input_standardizer::{
    Encoding, EnhancementOptions, RoleDecision, SegmentationDecision, StandardizeOptions,
    StandardizeResult, standardize,
};
use serde::{Deserialize, Serialize};
use typesafe_ai::{Client, Json};

/// `JEV_EVAL_DIR` picks another case set, relative to this package (default `evals`).
fn dir() -> String {
    let set = env::var("JEV_EVAL_DIR").unwrap_or_else(|_| "evals".to_owned());
    format!("{}/{set}", env!("CARGO_MANIFEST_DIR"))
}

#[derive(Deserialize)]
struct Case {
    id: String,
    host: String,
    target_model: String,
    draft: String,
    expect: Expect,
}

#[derive(Deserialize)]
struct Expect {
    encoding: Encoding,
    /// `None`: either is fine.
    notes: Option<bool>,
    #[serde(default)]
    keep: Vec<String>,
    /// Draft phrases and the roles each may be tagged with (`task|constraint`).
    #[serde(default)]
    tags: Vec<(String, String)>,
}

/// Points per case: a wrong tag misstates intent, so it costs more than lost structure.
const POINTS: usize = 6;

#[derive(Serialize)]
struct Outcome {
    id: String,
    passed: usize,
    failed: Vec<&'static str>,
    encoding: Encoding,
    lost: Vec<String>,
    output: String,
    /// Why the output looks the way it does, for diagnosing a failed check.
    roles: Vec<RoleDecision>,
    splits: Vec<SegmentationDecision>,
}

/// Lowercase words, so markup and punctuation changes never count as loss.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Draft words missing from the output that no applied removal or edit explains.
fn lost_words(draft: &str, output: &str, explained: &[&str]) -> Vec<String> {
    let bag = |texts: &[&str]| {
        let mut counts = HashMap::new();
        for word in texts.iter().flat_map(|text| words(text)) {
            *counts.entry(word).or_insert(0_usize) += 1;
        }
        counts
    };
    let mut present = bag(&[output]);
    let mut allowed = bag(explained);
    words(draft)
        .into_iter()
        .filter(|word| {
            let take = |counts: &mut HashMap<String, usize>| {
                counts
                    .get_mut(word)
                    .filter(|count| **count > 0)
                    .map(|count| *count -= 1)
                    .is_some()
            };
            !take(&mut present) && !take(&mut allowed)
        })
        .collect()
}

/// (role, text) per segment of the JSON rendering, which every prose format renders
/// from; `None` is untagged text.
fn segments(result: &StandardizeResult) -> Vec<(Option<String>, String)> {
    let json = result
        .alternatives
        .iter()
        .find(|alternative| alternative.encoding == Encoding::Json)
        .map_or("null", |alternative| alternative.text.as_str());
    let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let text = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_owned();
    if let Some(items) = value["segments"].as_array() {
        let role = |item: &serde_json::Value| item["role"].as_str().map(str::to_owned);
        return items
            .iter()
            .map(|item| (role(item), text(&item["text"])))
            .collect();
    }
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| *key != "standardizer_notes")
        .map(|(key, value)| (Some(key.clone()), text(value)))
        .collect()
}

/// Tagged output needs expected tags, and each anchor's segment untagged or tagged with
/// one of its roles. Plain output shows no tag; the encoding check counts lost structure.
fn tags_hold(
    tags: &[(String, String)],
    encoding: Encoding,
    segments: &[(Option<String>, String)],
) -> bool {
    let fits = |(anchor, roles): &(String, String)| {
        let anchor = anchor.to_lowercase();
        let found = segments
            .iter()
            .find(|(_, text)| text.to_lowercase().contains(&anchor));
        found.is_some_and(|(role, _)| allowed(role.as_deref(), roles))
    };
    encoding == Encoding::Plain || (!tags.is_empty() && tags.iter().all(fits))
}

/// Untagged, or tagged with one of `roles` (`task|constraint`).
fn allowed(role: Option<&str>, roles: &str) -> bool {
    role.is_none_or(|role| roles.split('|').any(|allowed| allowed == role))
}

fn score(case: &Case, result: &StandardizeResult) -> Outcome {
    let explained: Vec<&str> = result
        .filler_decisions
        .iter()
        .filter(|decision| decision.removed)
        .map(|decision| decision.phrase.as_str())
        .chain(
            result
                .edit_decisions
                .iter()
                .filter(|decision| decision.applied)
                .map(|decision| decision.original.as_str()),
        )
        .collect();
    let lost = lost_words(&case.draft, &result.text, &explained);
    let has_notes = result.text.contains("standardizer_notes");
    let checks = [
        ("lossless", lost.is_empty(), 1),
        ("encoding", result.encoding == case.expect.encoding, 1),
        (
            "notes",
            case.expect.notes.is_none_or(|want| want == has_notes),
            1,
        ),
        (
            "keep",
            case.expect
                .keep
                .iter()
                .all(|phrase| result.text.contains(phrase.as_str())),
            1,
        ),
        (
            "tags",
            tags_hold(&case.expect.tags, result.encoding, &segments(result)),
            2,
        ),
    ];
    Outcome {
        id: case.id.clone(),
        passed: checks
            .iter()
            .filter(|(_, ok, _)| *ok)
            .map(|(_, _, points)| points)
            .sum(),
        failed: checks
            .iter()
            .filter(|(_, ok, _)| !ok)
            .map(|(name, _, _)| *name)
            .collect(),
        encoding: result.encoding,
        lost,
        output: result.text.clone(),
        roles: result.role_decisions.clone(),
        splits: result.segmentation_decisions.clone(),
    }
}

#[tokio::test]
#[ignore = "requires TYPESAFE_API_KEY and makes live API requests"]
async fn golden_cases_hold_their_baseline() {
    let _ = dotenvy::dotenv();
    let client = Client::from_env().unwrap();
    let dir = dir();
    let cases: Vec<Case> = fs::read_to_string(format!("{dir}/cases.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut outcomes = Vec::new();
    for case in &cases {
        // The Alt+G editor's options (src/bin/editor.rs `request_options`), minus the
        // conversation it reads from the host session.
        let options = StandardizeOptions {
            host: Some(case.host.clone()),
            target_model: Some(case.target_model.clone()),
            enhancements: EnhancementOptions {
                correct_typos: true,
                rephrase: true,
                response_guidance: true,
            },
            jev_min_chars: 0,
            request_timeout_ms: 3_000,
            ..StandardizeOptions::default()
        };
        let result = standardize(&client, &Json::from(case.draft.as_str()), &options)
            .await
            .unwrap();
        let outcome = score(case, &result);
        eprintln!(
            "{:<18} {}/{POINTS} {:<9} {}{}",
            outcome.id,
            outcome.passed,
            format!("{:?}", outcome.encoding).to_lowercase(),
            outcome.failed.join(","),
            if outcome.lost.is_empty() {
                String::new()
            } else {
                format!(" lost={:?}", outcome.lost)
            },
        );
        outcomes.push(outcome);
    }
    let total: usize = outcomes.iter().map(|outcome| outcome.passed).sum();
    let scores: BTreeMap<&str, usize> = outcomes
        .iter()
        .map(|outcome| (outcome.id.as_str(), outcome.passed))
        .collect();
    eprintln!("total {total}/{}", POINTS * outcomes.len());
    // How often Jev is sure of a segment's role; below the bar the draft keeps its layout.
    let confidences: Vec<f64> = outcomes
        .iter()
        .flat_map(|outcome| &outcome.roles)
        .filter_map(|role| role.confidence)
        .collect();
    let confident = confidences
        .iter()
        .filter(|confidence| **confidence >= 0.8)
        .count();
    let (sum, count) = confidences
        .iter()
        .fold((0.0, 0.0), |(sum, count), confidence| {
            (sum + confidence, count + 1.0)
        });
    eprintln!(
        "roles confident {confident}/{} · mean {:.2}",
        confidences.len(),
        sum / f64::max(count, 1.0)
    );
    fs::create_dir_all(format!("{dir}/results")).unwrap();
    fs::write(
        format!("{dir}/results/latest.json"),
        serde_json::to_string_pretty(&outcomes).unwrap(),
    )
    .unwrap();
    let baseline_path = format!("{dir}/baseline.json");
    if env::var("JEV_EVAL_BASELINE").as_deref() == Ok("update") {
        fs::write(
            &baseline_path,
            serde_json::to_string_pretty(&scores).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
    let baseline: BTreeMap<String, usize> =
        serde_json::from_str(&fs::read_to_string(&baseline_path).unwrap()).unwrap();
    for (id, before) in &baseline {
        let now = scores.get(id.as_str()).copied().unwrap_or(0);
        if now < *before {
            eprintln!("regressed {id}: {before} -> {now}");
        }
    }
    let before: usize = baseline.values().sum();
    assert!(
        total >= before,
        "score {total} fell below baseline {before}"
    );
}

#[test]
fn lost_words_ignore_markup_and_explained_changes() {
    let draft = "Please fix the parser. Don't touch the API";
    let output =
        "<instructions>fix the parser</instructions>\n<constraints>Don't touch API</constraints>";
    assert_eq!(lost_words(draft, output, &["Please"]), ["the"]);
    assert!(lost_words(draft, output, &["Please", "the"]).is_empty());
    assert!(lost_words("a a", "a", &[]).len() == 1);
}

#[test]
fn tags_pass_untagged_text_and_allowed_roles_only() {
    let tags = [
        ("fix the parser".to_owned(), "task".to_owned()),
        ("check back".to_owned(), "context".to_owned()),
    ];
    let segment = |role: Option<&str>, text: &str| (role.map(str::to_owned), text.to_owned());
    let right = [
        segment(Some("task"), "Fix the parser."),
        segment(None, "I will check back."),
    ];
    let wrong = [segment(Some("task"), "Fix the parser. I will check back.")];
    assert!(tags_hold(&tags, Encoding::Xml, &right));
    assert!(!tags_hold(&tags, Encoding::Markdown, &wrong));
    assert!(!tags_hold(&tags, Encoding::Xml, &right[..1]));
    assert!(tags_hold(&tags, Encoding::Plain, &wrong));
    assert!(!tags_hold(&[], Encoding::Xml, &right));
}
