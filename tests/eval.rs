//! Golden-case eval: Alt+G drafts through the live standardizer, each scored
//! on four checks. Run it after any change that could move the output:
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
}

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
        ("lossless", lost.is_empty()),
        ("encoding", result.encoding == case.expect.encoding),
        (
            "notes",
            case.expect.notes.is_none_or(|want| want == has_notes),
        ),
        (
            "keep",
            case.expect
                .keep
                .iter()
                .all(|phrase| result.text.contains(phrase.as_str())),
        ),
    ];
    Outcome {
        id: case.id.clone(),
        passed: checks.iter().filter(|(_, ok)| *ok).count(),
        failed: checks
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(name, _)| *name)
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
            "{:<18} {}/4 {:<9} {}{}",
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
    eprintln!("total {total}/{}", 4 * outcomes.len());
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
