use typesafe_ai::{Json, SystemOneResponse};

use crate::draft::Draft;
use crate::model::{
    DecisionSource, EditDecision, FillerDecision, RoleDecision, SegmentRole, SegmentationDecision,
    StandardizeOptions,
};
use crate::segment::{
    ReplacementEdit, TextUnit, cleaned_structured, edited_units, looks_like_question,
    merge_root_units, root_context,
};

/// The draft after Jev's answers: code applies only the changes Jev approved.
pub(crate) struct Decided {
    pub standardized: Json,
    /// The prose with approved edits in its original layout; `None` for structured input.
    pub plain: Option<String>,
    pub segmentation_decisions: Vec<SegmentationDecision>,
    pub filler_decisions: Vec<FillerDecision>,
    pub edit_decisions: Vec<EditDecision>,
    pub role_decisions: Vec<RoleDecision>,
    pub segments: usize,
    pub filler_removed: usize,
    pub edits_applied: usize,
}

pub(crate) fn apply(
    draft: &Draft<'_>,
    response: Option<&SystemOneResponse>,
    options: &StandardizeOptions,
) -> Decided {
    let (unit_roles, role_decisions) = roles(draft, response, options.min_confidence);
    let (split, segmentation_decisions) = segmentation(draft, response, options, &role_decisions);
    let (remove, filler_decisions) = filler(draft, response, options);
    let (replacements, edit_decisions) = edits(draft, response, options.min_confidence);
    let cleaned = edited_units(&draft.units, &draft.filler, &remove, &replacements);
    let plain = draft.prose.then(|| {
        let joined = vec![false; draft.boundaries.len()];
        merge_root_units(&cleaned, &draft.boundaries, &joined)
            .0
            .concat()
    });
    let (texts, roles) = if draft.prose {
        let (texts, groups) = merge_root_units(&cleaned, &draft.boundaries, &split);
        let roles = grouped_roles(&groups, &unit_roles, &role_decisions);
        (texts, roles)
    } else {
        (cleaned, Vec::new())
    };
    Decided {
        standardized: rebuild_context(draft.source, &draft.units, &texts, &roles, draft.prose),
        plain,
        segmentation_decisions,
        filler_decisions,
        edit_decisions,
        role_decisions,
        segments: if draft.prose { texts.len() } else { 0 },
        filler_removed: remove.iter().filter(|remove| **remove).count(),
        edits_applied: replacements.len(),
    }
}

pub(crate) fn provisional_context(source: &Json, units: &[TextUnit], prose: bool) -> Json {
    let texts: Vec<_> = units.iter().map(|unit| unit.text.clone()).collect();
    rebuild_context(source, units, &texts, &heuristic_roles(units), prose)
}

fn rebuild_context(
    source: &Json,
    units: &[TextUnit],
    texts: &[String],
    roles: &[SegmentRole],
    prose: bool,
) -> Json {
    if prose {
        root_context(texts, roles)
    } else {
        cleaned_structured(source, units, texts)
    }
}

fn segmentation(
    draft: &Draft<'_>,
    response: Option<&SystemOneResponse>,
    options: &StandardizeOptions,
    roles: &[RoleDecision],
) -> (Vec<bool>, Vec<SegmentationDecision>) {
    draft
        .boundaries
        .iter()
        .map(|boundary| {
            let probability = response
                .and_then(|response| response.noul(&format!("split_{}", boundary.id)))
                .map(|answer| answer.noul);
            let confident = probability.filter(|value| {
                *value >= options.min_confidence || *value <= 1.0 - options.min_confidence
            });
            // Confidently different roles on each side are a boundary in themselves.
            let roles_differ = match (
                roles.get(boundary.left_unit_index),
                roles.get(boundary.right_unit_index),
            ) {
                (Some(left), Some(right)) => {
                    left.applied && right.applied && left.role != right.role
                }
                _ => false,
            };
            // Otherwise, uncertain either way keeps the source layout's own break.
            let split = match confident {
                Some(value) => value >= options.min_confidence,
                None => roles_differ || boundary.fallback_split,
            };
            let decision = SegmentationDecision {
                id: boundary.id.clone(),
                before: draft.units[boundary.left_unit_index].id.clone(),
                after: draft.units[boundary.right_unit_index].id.clone(),
                split_probability: probability,
                split,
                source: source_of(confident.is_some() || roles_differ),
            };
            (split, decision)
        })
        .unzip()
}

fn filler(
    draft: &Draft<'_>,
    response: Option<&SystemOneResponse>,
    options: &StandardizeOptions,
) -> (Vec<bool>, Vec<FillerDecision>) {
    draft
        .filler
        .iter()
        .map(|candidate| {
            let probability = response
                .and_then(|response| response.noul(&format!("drop_{}", candidate.id)))
                .map(|answer| answer.noul);
            let removed = probability.is_some_and(|value| value >= options.min_confidence);
            let decision = FillerDecision {
                id: candidate.id.clone(),
                path: draft.units[candidate.unit_index].path.clone(),
                phrase: candidate.phrase.clone(),
                remove_probability: probability,
                removed,
            };
            (removed, decision)
        })
        .unzip()
}

fn edits(
    draft: &Draft<'_>,
    response: Option<&SystemOneResponse>,
    min_confidence: f64,
) -> (Vec<ReplacementEdit>, Vec<EditDecision>) {
    let mut replacements = Vec::new();
    let mut decisions = Vec::with_capacity(draft.edits.len());
    for candidate in &draft.edits {
        let answer =
            response.and_then(|response| response.choice(&format!("edit_{}", candidate.id)));
        let selected = answer.and_then(|answer| {
            let index = answer.choice.strip_prefix('c')?.parse::<usize>().ok()?;
            candidate.choices.get(index.checked_sub(1)?)
        });
        let applied =
            selected.is_some() && answer.is_some_and(|answer| answer.confidence >= min_confidence);
        if let Some(replacement) = selected.filter(|_| applied) {
            replacements.push(ReplacementEdit {
                unit_index: candidate.unit_index,
                range: candidate.range.clone(),
                replacement: replacement.clone(),
            });
        }
        decisions.push(EditDecision {
            id: candidate.id.clone(),
            kind: candidate.kind.to_owned(),
            path: draft.units[candidate.unit_index].path.clone(),
            original: candidate.original.clone(),
            replacement: selected.cloned(),
            applied,
            confidence: answer.map(|answer| answer.confidence),
        });
    }
    (replacements, decisions)
}

fn roles(
    draft: &Draft<'_>,
    response: Option<&SystemOneResponse>,
    min_confidence: f64,
) -> (Vec<SegmentRole>, Vec<RoleDecision>) {
    if !draft.prose {
        return (Vec::new(), Vec::new());
    }
    draft
        .units
        .iter()
        .zip(heuristic_roles(&draft.units))
        .map(|(unit, fallback)| {
            let answer =
                response.and_then(|response| response.choice(&format!("role_{}", unit.id)));
            let answered = answer.and_then(|answer| SegmentRole::parse(&answer.choice));
            let confident = answer.is_some_and(|answer| answer.confidence >= min_confidence);
            let role = answered.filter(|_| confident).unwrap_or(fallback);
            let decision = RoleDecision {
                id: unit.id.clone(),
                role,
                answer: answered,
                confidence: answer.map(|answer| answer.confidence),
                applied: confident && answered.is_some(),
            };
            (role, decision)
        })
        .unzip()
}

/// A merged segment takes the role Jev was most confident about; ties keep the earliest.
fn grouped_roles(
    groups: &[Vec<usize>],
    roles: &[SegmentRole],
    decisions: &[RoleDecision],
) -> Vec<SegmentRole> {
    let confidence = |index: usize| decisions[index].confidence.unwrap_or(-1.0);
    groups
        .iter()
        .map(|group| {
            let best = group
                .iter()
                .rev()
                .max_by(|&&left, &&right| confidence(left).total_cmp(&confidence(right)))
                .copied()
                .unwrap_or(group[0]);
            roles[best]
        })
        .collect()
}

fn source_of(answered: bool) -> DecisionSource {
    if answered {
        DecisionSource::Jev
    } else {
        DecisionSource::Heuristic
    }
}

fn heuristic_roles(units: &[TextUnit]) -> Vec<SegmentRole> {
    units
        .iter()
        .map(|unit| heuristic_role(&unit.text))
        .collect()
}

fn heuristic_role(text: &str) -> SegmentRole {
    let lower = text.trim_start().to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| lower.starts_with(prefix));
    if looks_like_question(text)
        || looks_like_question(lower.strip_prefix("also ").unwrap_or(&lower))
    {
        SegmentRole::Question
    } else if starts(&["example", "e.g."]) {
        SegmentRole::Example
    } else if starts(&["output", "return"]) {
        SegmentRole::Output
    } else if starts(&["must", "do not", "don't", "constraint"]) {
        SegmentRole::Constraint
    } else if starts(&["input"]) {
        SegmentRole::Input
    } else {
        // Most unmarked sentences in an agent prompt are instructions, and a task
        // mistaken for background is worse than the reverse: it may go undone.
        SegmentRole::Task
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmarked_later_sentences_default_to_task() {
        assert_eq!(
            heuristic_role("then allow full screen chart as modal"),
            SegmentRole::Task
        );
        assert_eq!(heuristic_role("- elements resizable"), SegmentRole::Task);
        assert_eq!(
            heuristic_role("don't touch the API"),
            SegmentRole::Constraint
        );
    }

    #[test]
    fn merged_roles_prefer_confidence_then_the_earliest_segment() {
        let decision = |confidence| RoleDecision {
            id: String::new(),
            role: SegmentRole::Task,
            answer: None,
            confidence,
            applied: true,
        };
        let roles = [
            SegmentRole::Task,
            SegmentRole::Constraint,
            SegmentRole::Question,
        ];
        let decisions = [decision(Some(0.9)), decision(Some(0.9)), decision(None)];

        assert_eq!(
            grouped_roles(&[vec![0, 1, 2]], &roles, &decisions),
            [SegmentRole::Task]
        );
        let decisions = [decision(Some(0.2)), decision(Some(0.9)), decision(None)];
        assert_eq!(
            grouped_roles(&[vec![0, 1], vec![2]], &roles, &decisions),
            [SegmentRole::Constraint, SegmentRole::Question]
        );
    }
}
