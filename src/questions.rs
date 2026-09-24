use std::collections::BTreeMap;

use serde::Serialize;
use typesafe_ai::{Choice, Json, Noul, NoulCriteria, Question, Questions};

use crate::context::{item_id, quotable};
use crate::draft::Draft;
use crate::edits::EditCandidate;
use crate::encoding::Candidates;
use crate::model::{BackgroundItem, StandardizeOptions};
use crate::segment::{BoundaryCandidate, FillerCandidate, TextUnit};
use crate::tokens::estimate_tokens;

/// Jev state budget for background; the newest items win.
const MAX_BACKGROUND_CHARS: usize = 24_000;

#[derive(Serialize)]
pub(crate) struct JudgmentState<'a> {
    purpose: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_model: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    background: Vec<StateItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_instruction: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skill_references: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a Json>,
    units: Vec<StateUnit<'a>>,
    boundary_candidates: Vec<StateBoundary<'a>>,
    filler_candidates: Vec<StateCandidate<'a>>,
    edit_candidates: Vec<StateEdit<'a>>,
    /// Estimated tokens per encoding, sent only when Jev votes on a structured-data encoding.
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding_tokens: Option<EncodingTokens>,
}

#[derive(Serialize)]
struct StateItem<'a> {
    id: String,
    source: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct StateUnit<'a> {
    id: &'a str,
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
}

#[derive(Serialize)]
struct StateCandidate<'a> {
    id: &'a str,
    unit: &'a str,
    phrase: &'a str,
}

#[derive(Serialize)]
struct StateBoundary<'a> {
    id: &'a str,
    before: &'a str,
    after: &'a str,
    cue: &'static str,
}

#[derive(Serialize)]
struct StateEdit<'a> {
    id: &'a str,
    unit: &'a str,
    kind: &'a str,
    original: &'a str,
    alternatives: &'a [String],
}

#[derive(Serialize)]
struct EncodingTokens {
    json: usize,
    toon: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    csv: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    xml: Option<usize>,
}

/// Keeps the newest items whose combined text fits the Jev state budget.
pub(crate) fn recent_background(items: &[BackgroundItem]) -> &[BackgroundItem] {
    let mut total = 0;
    let kept = items
        .iter()
        .rev()
        .take_while(|item| {
            total += item.text.chars().count();
            total <= MAX_BACKGROUND_CHARS
        })
        .count();
    &items[items.len() - kept..]
}

pub(crate) fn state<'a>(
    draft: &'a Draft<'_>,
    data: Option<&Candidates>,
    background: &'a [BackgroundItem],
    options: &'a StandardizeOptions,
) -> JudgmentState<'a> {
    let units = &draft.units;
    JudgmentState {
        purpose: "Prepare the user's draft for the target model without losing intent: choose segments, roles, edits, answer detail, research need, and encoding; preserve meaning and technical references. background is earlier conversation and other supplied material for resolving references only; judge only the draft units.",
        host: options
            .harness()
            .map(|harness| harness.name)
            .or(options.host.as_deref()),
        target_model: options.target_model.as_deref(),
        background: background
            .iter()
            .enumerate()
            .map(|(index, item)| StateItem {
                id: item_id(index),
                source: &item.source,
                text: &item.text,
            })
            .collect(),
        host_instruction: (!draft.skill_references.is_empty()).then_some(
            "Host-native references may invoke skills. Preserve them verbatim and keep them obvious in the selected encoding.",
        ),
        skill_references: draft.skill_references.iter().map(String::as_str).collect(),
        context: (!draft.prose).then_some(&draft.provisional),
        units: units
            .iter()
            .map(|unit| StateUnit {
                id: &unit.id,
                path: &unit.path,
                text: draft.prose.then_some(unit.text.as_str()),
            })
            .collect(),
        boundary_candidates: draft
            .boundaries
            .iter()
            .map(|boundary| StateBoundary {
                id: &boundary.id,
                before: &units[boundary.left_unit_index].id,
                after: &units[boundary.right_unit_index].id,
                cue: boundary.cue,
            })
            .collect(),
        filler_candidates: draft
            .filler
            .iter()
            .map(|candidate| StateCandidate {
                id: &candidate.id,
                unit: &units[candidate.unit_index].id,
                phrase: &candidate.phrase,
            })
            .collect(),
        edit_candidates: draft
            .edits
            .iter()
            .map(|edit| StateEdit {
                id: &edit.id,
                unit: &units[edit.unit_index].id,
                kind: edit.kind,
                original: &edit.original,
                alternatives: &edit.choices,
            })
            .collect(),
        encoding_tokens: data.map(|data| EncodingTokens {
            json: estimate_tokens(&data.json),
            toon: estimate_tokens(&data.toon),
            csv: data.csv.as_deref().map(estimate_tokens),
            xml: data.xml.as_deref().map(estimate_tokens),
        }),
    }
}

/// `data` is present only when Jev should vote on a structured-data encoding.
pub(crate) fn questions(
    draft: &Draft<'_>,
    data: Option<&Candidates>,
    background: &[BackgroundItem],
    options: &StandardizeOptions,
) -> Questions {
    let mut questions = Questions::new();
    if let Some(data) = data {
        let available = (data.csv.is_some(), data.xml.is_some());
        questions.insert("encoding".to_owned(), encoding_question(available));
    }
    let quotable = quotable(background);
    if draft.prose && !quotable.is_empty() {
        let ids: Vec<&str> = quotable.iter().map(|(id, _)| id.as_str()).collect();
        questions.insert("context".to_owned(), context_question(&ids));
    }
    if draft.prose {
        for boundary in &draft.boundaries {
            let question = segmentation_question(boundary, &draft.units);
            questions.insert(format!("split_{}", boundary.id), question);
        }
        for unit in &draft.units {
            questions.insert(format!("role_{}", unit.id), role_question(unit));
        }
    }
    for candidate in &draft.filler {
        questions.insert(format!("drop_{}", candidate.id), filler_question(candidate));
    }
    for edit in &draft.edits {
        questions.insert(format!("edit_{}", edit.id), edit_question(edit));
    }
    if options.enhancements.response_guidance && draft.prose {
        questions.insert("answer_style".to_owned(), answer_style_question());
        questions.insert("research".to_owned(), research_question());
    }
    questions
}

fn context_question(ids: &[&str]) -> Question {
    let mut criteria = BTreeMap::from([(
        "none".to_owned(),
        Some(Json::from("The draft is understandable on its own.")),
    )]);
    for id in ids {
        let description = format!("The draft depends on background item {id}.");
        criteria.insert((*id).to_owned(), Some(Json::from(description)));
    }
    Question::Choice(Choice::new(criteria).instructions(
        "Which outside background item (not a conversation turn) does the user's draft most depend on? For example it answers a question or picks an option offered there, or says 'both', 'this page', or 'that fix'. Choose none when the draft is self-contained.",
    ))
}

fn edit_question(edit: &EditCandidate) -> Question {
    let mut criteria = BTreeMap::from([(
        "keep".to_owned(),
        Some(Json::from("Keep the user's exact original wording.")),
    )]);
    for (index, choice) in edit.choices.iter().enumerate() {
        criteria.insert(
            format!("c{}", index + 1),
            Some(Json::from(format!("Replace with `{choice}`."))),
        );
    }
    let instruction = if edit.kind == "question" {
        format!(
            "For `{}` in this question, prefer an obvious capitalization or question-mark correction unless the original has technical meaning. Keep only when the change would be misleading.",
            edit.original
        )
    } else {
        format!(
            "For `{}` ({}) in its full prompt context, choose a correction only if it preserves the intended meaning, technical terms, and tone; otherwise keep.",
            edit.original, edit.kind
        )
    };
    Question::Choice(Choice::new(criteria).instructions(instruction))
}

fn answer_style_question() -> Question {
    Question::Choice(
        Choice::new(BTreeMap::from([
            (
                "concise".to_owned(),
                Some(Json::from("Give a short direct answer; retain required details, results, and caveats.")),
            ),
            (
                "standard".to_owned(),
                Some(Json::from("Normal detail or the user specified an exact format; add no extra style constraint.")),
            ),
            (
                "detailed".to_owned(),
                Some(Json::from("The user explicitly needs thorough explanation, comparison, or teaching.")),
            ),
        ]))
        .instructions("What answer length best fits the user's task and explicit output requirements?"),
    )
}

fn research_question() -> Question {
    Question::Choice(
        Choice::new(BTreeMap::from([
            (
                "none".to_owned(),
                Some(Json::from("The supplied context suffices; do not add a research instruction.")),
            ),
            (
                "local".to_owned(),
                Some(Json::from("The task needs inspection of relevant project files or local documentation.")),
            ),
            (
                "web".to_owned(),
                Some(Json::from("The task explicitly calls for current or external facts that require checking authoritative sources.")),
            ),
        ]))
        .instructions("Should the downstream agent research before answering? Choose none when background already covers what the draft needs. This decision only flags research; Jev does not fetch sources."),
    )
}

fn segmentation_question(boundary: &BoundaryCandidate, units: &[TextUnit]) -> Question {
    Question::Noul(
        Noul::new(format!(
            "Should `{}` start a distinct semantic segment after `{}`?",
            units[boundary.right_unit_index].id, units[boundary.left_unit_index].id
        ))
        .criteria(
            NoulCriteria::new()
                .yes("A distinct question, task, constraint, input, output, example, or separate context; especially separate a follow-up question from an action request.")
                .no("Continues the same thought, code block, or semantic role."),
        ),
    )
}

fn encoding_question((csv_available, xml_available): (bool, bool)) -> Question {
    let mut criteria = BTreeMap::from([
        (
            "json".to_owned(),
            Some(Json::from(
                "Data-like input where explicit keys or nesting matter.",
            )),
        ),
        (
            "toon".to_owned(),
            Some(Json::from("Compact form for uniform structured records.")),
        ),
    ]);
    if csv_available {
        criteria.insert(
            "csv".to_owned(),
            Some(Json::from("Unambiguous flat table of scalar records.")),
        );
    }
    if xml_available {
        criteria.insert(
            "xml".to_owned(),
            Some(Json::from("Prose instructions as tagged sections; Anthropic and OpenAI prompt guides recommend XML tags to separate tasks, constraints, and context, while JSON escaping hurts readability.")),
        );
    }
    Question::Choice(Choice::new(criteria).instructions(
        "Which encoding will target_model follow most reliably? Prefer readability for prose instructions; use encoding_tokens only to break ties.",
    ))
}

fn role_question(unit: &TextUnit) -> Question {
    // Each description says what the agent should do with the part, which is what
    // separates the roles Jev confuses most: task, constraint, input, and context.
    let criteria = [
        (
            "task",
            "Something the user wants the agent to do or change, including follow-ups that start with then, also, and, or ensure.",
        ),
        (
            "question",
            "Asks the agent for an answer or explanation rather than an action.",
        ),
        (
            "context",
            "Background the agent should know, such as current state, what happened, why, or what the user will do themselves; nothing for the agent to do.",
        ),
        (
            "constraint",
            "Limits how a task is done: a requirement, preference, or prohibition on the work, such as must, don't, only, or should be.",
        ),
        (
            "input",
            "Material the user pasted for the agent to work on: data, logs, error output, code, or quoted text.",
        ),
        (
            "output",
            "The shape, format, length, or style wanted for the reply or result.",
        ),
        (
            "example",
            "An illustration of what is wanted, such as a sample, a reference to copy, or an attached image to match.",
        ),
    ]
    .into_iter()
    .map(|(role, text)| (role.to_owned(), Some(Json::from(text))))
    .collect::<BTreeMap<_, _>>();
    Question::Choice(Choice::new(criteria).instructions(format!(
        "Primary role of `{}` in the user's draft? Judge it by what the user wants the agent to do with that part.",
        unit.path
    )))
}

fn filler_question(candidate: &FillerCandidate) -> Question {
    Question::Noul(
        Noul::new(format!(
            "Can `{}` be deleted without changing meaning, tone, quotation, or code?",
            candidate.id
        ))
        .criteria(
            NoulCriteria::new()
                .yes("Pure conversational filler.")
                .no("Meaningful, quoted, code, or uncertain."),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_background_keeps_the_newest_items_within_budget() {
        let item = |text: String| BackgroundItem {
            source: "user".to_owned(),
            text,
        };
        let items = vec![
            item("o".repeat(MAX_BACKGROUND_CHARS / 2)),
            item("a".repeat(MAX_BACKGROUND_CHARS / 2)),
            item("b".repeat(MAX_BACKGROUND_CHARS / 4)),
        ];

        let kept = recent_background(&items);
        assert_eq!(kept.len(), 2);
        assert!(kept[0].text.starts_with('a'));
        assert!(recent_background(&[]).is_empty());
    }
}
