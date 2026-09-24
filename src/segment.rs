use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::ops::Range;

use typesafe_ai::Json;

use crate::harness::SkillSyntax;
use crate::model::SegmentRole;

mod boundary;

pub(crate) use boundary::{QUESTION_STARTS, looks_like_question};

// Only politeness and framing. "just" (only), "if possible" (optional), "actually"
// (correction), "simply", "can/could/would you" (question vs. command) and "when you
// get a chance" (priority) can carry intent, so they are never deletion candidates.
const FILLER_PHRASES: [&str; 6] = [
    "i would like you to",
    "i'd like you to",
    "i want you to",
    "basically",
    "kindly",
    "please",
];

#[derive(Clone, Debug)]
pub(crate) enum PathPart {
    Key(String),
    Index(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct TextUnit {
    pub id: String,
    pub path: String,
    pub text: String,
    pub locator: Vec<PathPart>,
}

#[derive(Clone, Debug)]
pub(crate) struct BoundaryCandidate {
    pub id: String,
    pub left_unit_index: usize,
    pub right_unit_index: usize,
    pub separator: String,
    pub cue: &'static str,
    pub fallback_split: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct FillerCandidate {
    pub id: String,
    pub unit_index: usize,
    pub range: Range<usize>,
    pub phrase: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ReplacementEdit {
    pub unit_index: usize,
    pub range: Range<usize>,
    pub replacement: String,
}

pub(crate) fn root_units(
    text: &str,
    max_segments: usize,
) -> (Vec<TextUnit>, Vec<BoundaryCandidate>) {
    let breaks = candidate_breaks(text, max_segments.saturating_sub(1));
    let mut units = Vec::with_capacity(breaks.len() + 1);
    let mut boundaries = Vec::with_capacity(breaks.len());
    let mut start = 0;

    for (index, candidate) in breaks.iter().enumerate() {
        units.push(root_unit(index, &text[start..candidate.left_end]));
        boundaries.push(BoundaryCandidate {
            id: format!("b{}", index + 1),
            left_unit_index: index,
            right_unit_index: index + 1,
            separator: text[candidate.left_end..candidate.right_start].to_owned(),
            cue: candidate.cue,
            fallback_split: candidate.fallback_split,
        });
        start = candidate.right_start;
    }
    units.push(root_unit(units.len(), &text[start..]));
    (units, boundaries)
}

pub(crate) fn structured_units(value: &Json) -> Vec<TextUnit> {
    let mut units = Vec::new();
    collect_strings(value, &mut Vec::new(), &mut units);
    units
}

pub(crate) fn skill_references(units: &[TextUnit], syntax: SkillSyntax) -> Vec<String> {
    let mut references = BTreeSet::new();
    for unit in units {
        let literal = literal_mask(&unit.text);
        let spans = match syntax {
            SkillSyntax::Dollar => dollar_skill_reference_spans(&unit.text),
            SkillSyntax::Slash => slash_skill_reference_spans(&unit.text),
            SkillSyntax::None => Vec::new(),
        };
        for range in spans {
            if literal[range.clone()].iter().any(|protected| *protected) {
                continue;
            }
            references.insert(unit.text[range].to_owned());
        }
    }
    references.into_iter().collect()
}

pub(crate) fn filler_candidates(units: &[TextUnit], maximum: usize) -> Vec<FillerCandidate> {
    units
        .iter()
        .enumerate()
        .flat_map(|(unit_index, unit)| {
            filler_ranges(&unit.text)
                .into_iter()
                .map(move |range| (unit_index, unit, range))
        })
        .take(maximum)
        .enumerate()
        .map(|(index, (unit_index, unit, range))| FillerCandidate {
            id: format!("f{}", index + 1),
            unit_index,
            phrase: unit.text[range.clone()].to_owned(),
            range,
        })
        .collect()
}

/// Non-overlapping filler phrases outside code, in phrase-list order.
fn filler_ranges(text: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let protected = code_mask(text);
    let mut taken: Vec<Range<usize>> = Vec::new();
    for phrase in FILLER_PHRASES {
        for (start, _) in lower.match_indices(phrase) {
            let range = start..start + phrase.len();
            let free = is_boundary(&lower, range.start, range.end)
                && !protected[range.clone()].iter().any(|value| *value)
                && !taken
                    .iter()
                    .any(|other| range.start < other.end && range.end > other.start);
            if free {
                taken.push(range);
            }
        }
    }
    taken
}

pub(crate) fn edited_units(
    units: &[TextUnit],
    candidates: &[FillerCandidate],
    remove: &[bool],
    edits: &[ReplacementEdit],
) -> Vec<String> {
    units
        .iter()
        .enumerate()
        .map(|(unit_index, unit)| {
            let mut changes: Vec<_> = candidates
                .iter()
                .zip(remove)
                .filter(|(candidate, remove)| candidate.unit_index == unit_index && **remove)
                .map(|(candidate, _)| (candidate.range.clone(), ""))
                .collect();
            changes.extend(
                edits
                    .iter()
                    .filter(|edit| edit.unit_index == unit_index)
                    .map(|edit| (edit.range.clone(), edit.replacement.as_str())),
            );
            if changes.is_empty() {
                return unit.text.clone();
            }
            changes.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
            let mut text = unit.text.clone();
            let removed_at_start = changes
                .iter()
                .any(|(range, replacement)| range.start == 0 && replacement.is_empty());
            for (range, replacement) in changes {
                text.replace_range(range, replacement);
            }
            if removed_at_start {
                text.trim_start_matches([' ', '\t', ',', ';']).to_owned()
            } else {
                text
            }
        })
        .collect()
}

pub(crate) fn merge_root_units(
    cleaned: &[String],
    boundaries: &[BoundaryCandidate],
    split: &[bool],
) -> (Vec<String>, Vec<Vec<usize>>) {
    let Some(first) = cleaned.first() else {
        return (Vec::new(), Vec::new());
    };
    let mut texts = vec![first.clone()];
    let mut groups = vec![vec![0]];
    for (boundary, split) in boundaries.iter().zip(split) {
        let next = boundary.right_unit_index;
        if *split {
            texts.push(cleaned[next].clone());
            groups.push(vec![next]);
        } else {
            let text = texts.last_mut().expect("the first root unit exists");
            text.push_str(&boundary.separator);
            text.push_str(&cleaned[next]);
            groups
                .last_mut()
                .expect("the first root group exists")
                .push(next);
        }
    }
    (texts, groups)
}

pub(crate) fn root_context(texts: &[String], roles: &[Option<SegmentRole>]) -> Json {
    if let ([text], [Some(role)]) = (texts, roles) {
        return Json::Object(BTreeMap::from([(
            role.as_str().to_owned(),
            Json::from(text.clone()),
        )]));
    }
    let segments = texts
        .iter()
        .zip(roles)
        .map(|(text, role)| {
            Json::Object(BTreeMap::from([
                // `null`: Jev confirmed no role, so no tag names one.
                (
                    "role".to_owned(),
                    role.map_or(Json::Null, |role| Json::from(role.as_str())),
                ),
                ("text".to_owned(), Json::from(text.clone())),
            ]))
        })
        .collect::<Vec<_>>();
    Json::Object(BTreeMap::from([(
        "segments".to_owned(),
        Json::Array(segments),
    )]))
}

pub(crate) fn cleaned_structured(original: &Json, units: &[TextUnit], cleaned: &[String]) -> Json {
    let mut value = original.clone();
    for (unit, text) in units.iter().zip(cleaned) {
        if let Some(Json::String(target)) = get_mut(&mut value, &unit.locator) {
            target.clone_from(text);
        }
    }
    value
}

fn root_unit(index: usize, text: &str) -> TextUnit {
    TextUnit {
        id: format!("s{}", index + 1),
        path: format!("segments[{index}].text"),
        text: text.trim().to_owned(),
        locator: Vec::new(),
    }
}

struct CandidateBreak {
    left_end: usize,
    right_start: usize,
    cue: &'static str,
    fallback_split: bool,
}

fn candidate_breaks(text: &str, maximum: usize) -> Vec<CandidateBreak> {
    if maximum == 0 {
        return Vec::new();
    }
    let protected = code_mask(text);
    let mut candidates = Vec::new();
    let mut characters = text.char_indices().peekable();
    while let Some((start, character)) = characters.next() {
        if !character.is_whitespace() || protected.get(start).copied().unwrap_or(false) {
            continue;
        }
        let mut end = start + character.len_utf8();
        let mut newlines = usize::from(character == '\n');
        while let Some(&(offset, next)) = characters.peek() {
            if !next.is_whitespace() || protected.get(offset).copied().unwrap_or(false) {
                break;
            }
            let _ = characters.next();
            end = offset + next.len_utf8();
            newlines += usize::from(next == '\n');
        }

        let left = text[..start].trim_end();
        let right = text[end..].trim_start();
        if left.is_empty() || right.is_empty() {
            continue;
        }
        let list_item = newlines > 0 && is_list_item(right);
        let terminal = left
            .chars()
            .next_back()
            .is_some_and(|value| matches!(value, '.' | '!' | '?' | ';' | ':' | '…'));
        let question_clause = boundary::question_clause_after_comma(left, right);
        let (cue, fallback_split) = if newlines >= 2 {
            ("blank_line", true)
        } else if list_item {
            ("list_item", true)
        } else if newlines > 0 {
            ("line_break", false)
        } else if terminal {
            ("sentence_end", false)
        } else if question_clause {
            ("question_clause", true)
        } else {
            continue;
        };
        candidates.push(CandidateBreak {
            left_end: if question_clause { start - 1 } else { start },
            right_start: end,
            cue,
            fallback_split,
        });
        if candidates.len() == maximum {
            break;
        }
    }
    candidates
}

fn is_list_item(text: &str) -> bool {
    text.starts_with("- ")
        || text.starts_with("* ")
        || text
            .split_once(". ")
            .is_some_and(|(prefix, _)| prefix.chars().all(|character| character.is_ascii_digit()))
}

fn collect_strings(value: &Json, path: &mut Vec<PathPart>, output: &mut Vec<TextUnit>) {
    match value {
        Json::String(text) => {
            output.push(TextUnit {
                id: format!("v{}", output.len() + 1),
                path: display_path(path),
                text: text.clone(),
                locator: path.clone(),
            });
        }
        Json::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                path.push(PathPart::Index(index));
                collect_strings(value, path, output);
                path.pop();
            }
        }
        Json::Object(values) => {
            for (key, value) in values {
                path.push(PathPart::Key(key.clone()));
                collect_strings(value, path, output);
                path.pop();
            }
        }
        Json::Null | Json::Bool(_) | Json::I64(_) | Json::U64(_) | Json::F64(_) => {}
    }
}

fn display_path(path: &[PathPart]) -> String {
    let mut result = "$".to_owned();
    for part in path {
        match part {
            PathPart::Key(key) => {
                result.push('.');
                result.push_str(key);
            }
            PathPart::Index(index) => {
                let _ = write!(result, "[{index}]");
            }
        }
    }
    result
}

fn get_mut<'a>(value: &'a mut Json, path: &[PathPart]) -> Option<&'a mut Json> {
    let mut current = value;
    for part in path {
        current = match (current, part) {
            (Json::Object(values), PathPart::Key(key)) => values.get_mut(key)?,
            (Json::Array(values), PathPart::Index(index)) => values.get_mut(*index)?,
            _ => return None,
        };
    }
    Some(current)
}

fn is_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    before.is_none_or(|character| !character.is_ascii_alphanumeric())
        && after.is_none_or(|character| !character.is_ascii_alphanumeric())
}

pub(crate) fn code_mask(text: &str) -> Vec<bool> {
    let mut mask = literal_mask(text);
    for range in dollar_skill_reference_spans(text)
        .into_iter()
        .chain(slash_skill_reference_spans(text))
    {
        for cell in &mut mask[range] {
            *cell = true;
        }
    }
    mask
}

fn literal_mask(text: &str) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut mask = vec![false; bytes.len()];
    let mut index = 0;
    let mut fenced = false;
    let mut inline = false;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"```") {
            for cell in mask.iter_mut().skip(index).take(3) {
                *cell = true;
            }
            fenced = !fenced;
            index += 3;
            continue;
        }
        if bytes[index] == b'`' && !fenced {
            mask[index] = true;
            inline = !inline;
            index += 1;
            continue;
        }
        mask[index] = fenced || inline;
        index += 1;
    }
    mask
}

fn dollar_skill_reference_spans(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] != b'$'
            || !bytes[index + 1].is_ascii_lowercase()
            || (index > 0
                && (bytes[index - 1].is_ascii_alphanumeric()
                    || matches!(bytes[index - 1], b'_' | b'\\')))
        {
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        while index < bytes.len()
            && (bytes[index].is_ascii_lowercase()
                || bytes[index].is_ascii_digit()
                || matches!(bytes[index], b'-' | b'_'))
        {
            index += 1;
        }
        ranges.push(start..index);
    }
    ranges
}

fn slash_skill_reference_spans(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] != b'/'
            || !bytes[index + 1].is_ascii_lowercase()
            || (index > 0 && !bytes[index - 1].is_ascii_whitespace())
        {
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        while index < bytes.len()
            && (bytes[index].is_ascii_lowercase()
                || bytes[index].is_ascii_digit()
                || matches!(bytes[index], b'-' | b'_'))
        {
            index += 1;
        }
        if index == bytes.len() || bytes[index].is_ascii_whitespace() {
            ranges.push(start..index);
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filler_detection_ignores_inline_and_fenced_code() {
        let (units, _) = root_units(
            "Please update `please.rs`.\n\n```text\nplease keep this literal\n```",
            8,
        );
        let candidates = filler_candidates(&units, 8);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].phrase, "Please");
    }

    #[test]
    fn intent_bearing_words_are_never_filler_candidates() {
        let (units, _) = root_units(
            "Can you just fix the header, if possible? Actually, simply keep it. Please add tests.",
            8,
        );
        let phrases: Vec<_> = filler_candidates(&units, 16)
            .into_iter()
            .map(|candidate| candidate.phrase)
            .collect();

        assert_eq!(phrases, ["Please"]);
    }

    #[test]
    fn one_segment_uses_a_compact_role_key() {
        let context = root_context(
            &["Compile the workspace".to_owned()],
            &[Some(SegmentRole::Task)],
        );
        let encoded = serde_json::to_string(&context).unwrap();

        assert_eq!(encoded, r#"{"task":"Compile the workspace"}"#);
    }

    #[test]
    fn sentence_end_is_a_model_boundary_candidate() {
        let (units, boundaries) = root_units(
            "Implement the parser. Do not change `parser.rs`.\n\nReturn JSON.",
            8,
        );

        assert_eq!(units.len(), 3);
        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].cue, "sentence_end");
        assert!(!boundaries[0].fallback_split);
        assert_eq!(boundaries[1].cue, "blank_line");
        assert!(boundaries[1].fallback_split);
    }

    #[test]
    fn comma_question_clause_is_a_distinct_candidate() {
        let (units, boundaries) = root_units(
            "Don't show the mux control pane as 4, show as 0, also how do we pin a tab now?",
            8,
        );

        assert_eq!(units.len(), 2);
        assert_eq!(
            units[0].text,
            "Don't show the mux control pane as 4, show as 0"
        );
        assert_eq!(units[1].text, "also how do we pin a tab now?");
        assert_eq!(boundaries[0].separator, ", ");
        assert_eq!(boundaries[0].cue, "question_clause");
        assert!(boundaries[0].fallback_split);
    }

    #[test]
    fn merging_preserves_the_original_boundary_separator() {
        let (units, boundaries) = root_units("First sentence. Second sentence.", 8);
        let cleaned = units
            .iter()
            .map(|unit| unit.text.clone())
            .collect::<Vec<_>>();
        let (texts, groups) = merge_root_units(&cleaned, &boundaries, &[false]);

        assert_eq!(texts, ["First sentence. Second sentence."]);
        assert_eq!(groups, [vec![0, 1]]);
    }

    #[test]
    fn no_filler_removal_preserves_exact_structured_text() {
        let units = structured_units(&Json::from(";  keep  spaces\tand tabs  "));
        let cleaned = edited_units(&units, &[], &[], &[]);
        assert_eq!(cleaned, [";  keep  spaces\tand tabs  "]);
    }

    #[test]
    fn filler_removal_does_not_reformat_unrelated_code() {
        let units = structured_units(&Json::from("Please check `a  ==  b` and\tkeep  spacing"));
        let candidates = filler_candidates(&units, 8);
        let cleaned = edited_units(&units, &candidates, &[true], &[]);
        assert_eq!(cleaned, ["check `a  ==  b` and\tkeep  spacing"]);
    }

    #[test]
    fn skill_invocations_are_excluded_from_filler_candidates() {
        let units = structured_units(&Json::from(
            "Use $just and $web-design-guidelines; please review.",
        ));
        let candidates = filler_candidates(&units, 8);
        assert_eq!(
            skill_references(&units, SkillSyntax::Dollar),
            ["$just", "$web-design-guidelines"]
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].phrase, "please");
    }

    #[test]
    fn claude_skill_invocations_are_host_specific_and_protected() {
        let units = structured_units(&Json::from("/just review /home/user and /deploy next"));
        assert_eq!(
            skill_references(&units, SkillSyntax::Slash),
            ["/deploy", "/just"]
        );
        assert!(filler_candidates(&units, 8).is_empty());
    }

    #[test]
    fn quoted_and_escaped_skill_names_are_not_inferred_as_invocations() {
        let units = structured_units(&Json::from(
            "Use `$web-design-guidelines` literally and \\$just.",
        ));
        assert!(skill_references(&units, SkillSyntax::Dollar).is_empty());
    }
}
