use typesafe_ai::{Json, SystemOneResponse};

use crate::model::GuidanceDecision;

/// Groups text the standardizer adds so the downstream model can tell it from the user's words.
pub(crate) const NOTES_KEY: &str = "standardizer_notes";

pub(crate) fn add_note(value: &mut Json, key: &str, text: String) {
    let Json::Object(fields) = value else { return };
    let notes = fields
        .entry(NOTES_KEY.to_owned())
        .or_insert_with(|| Json::Object(std::collections::BTreeMap::new()));
    if let Json::Object(notes) = notes {
        notes.insert(key.to_owned(), Json::from(text));
    }
}

pub(crate) fn apply(
    value: &mut Json,
    response: Option<&SystemOneResponse>,
    enabled: bool,
    min_confidence: f64,
) -> (Option<GuidanceDecision>, Option<GuidanceDecision>) {
    if !enabled {
        return (None, None);
    }
    let decide =
        |id, allowed: &[&str], fallback| decision(response, id, allowed, fallback, min_confidence);
    let answer = decide(
        "answer_style",
        &["concise", "standard", "detailed"],
        "standard",
    );
    let research = decide("research", &["none", "local", "web"], "none");
    let applied = |decision: &GuidanceDecision| {
        if decision.applied {
            decision.choice.clone()
        } else {
            String::new()
        }
    };
    insert(value, &applied(&answer), &applied(&research));
    (Some(answer), Some(research))
}

/// Jev's valid top answer, applied only at `min_confidence` or higher.
fn decision(
    response: Option<&SystemOneResponse>,
    id: &str,
    allowed: &[&str],
    fallback: &str,
    min_confidence: f64,
) -> GuidanceDecision {
    let answer = response
        .and_then(|response| response.choice(id))
        .filter(|answer| allowed.contains(&answer.choice.as_str()));
    GuidanceDecision {
        choice: answer
            .map_or(fallback, |answer| answer.choice.as_str())
            .to_owned(),
        confidence: answer.map(|answer| answer.confidence),
        applied: answer.is_some_and(|answer| answer.confidence >= min_confidence),
    }
}

fn insert(value: &mut Json, style: &str, research: &str) {
    let style = match style {
        "concise" => Some("Be concise; retain requested detail, results, and important caveats."),
        "detailed" => Some("Explain thoroughly as requested."),
        _ => None,
    };
    let research = match research {
        "local" => Some("Inspect relevant local project files or documentation before answering."),
        "web" => Some("Verify current external facts using authoritative sources and cite them."),
        _ => None,
    };
    for (key, text) in [("answer_style", style), ("research", research)] {
        if let Some(text) = text {
            add_note(value, key, text.to_owned());
        }
    }
}
