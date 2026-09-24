use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::ops::Range;
use std::process::{Command, Stdio};

use crate::model::EnhancementOptions;
use crate::segment::{FillerCandidate, TextUnit, code_mask};

#[path = "edits/question.rs"]
mod question;

const MAX_SPELL_WORDS: usize = 96;
const MAX_TYPO_CANDIDATES: usize = 8;
const MAX_PHRASE_CANDIDATES: usize = 6;
const PHRASES: [(&str, &str); 4] = [
    ("in order to", "to"),
    ("due to the fact that", "because"),
    ("at this point in time", "now"),
    ("with regard to", "about"),
];
const TECHNICAL_WORDS: [&str; 12] = [
    "api", "cli", "codex", "claude", "csv", "jev", "json", "nvim", "rust", "sdk", "toon", "wezterm",
];

#[derive(Clone, Debug)]
pub(crate) struct EditCandidate {
    pub id: String,
    pub kind: &'static str,
    pub unit_index: usize,
    pub range: Range<usize>,
    pub original: String,
    pub choices: Vec<String>,
}

pub(crate) fn candidates(
    units: &[TextUnit],
    filler: &[FillerCandidate],
    options: &EnhancementOptions,
) -> Vec<EditCandidate> {
    let mut edits = Vec::new();
    if options.rephrase {
        edits = phrase_candidates(units, filler);
        let questions = question::candidates(units, filler, &edits);
        edits.extend(questions);
    }
    if options.correct_typos {
        let typos = typo_candidates(units, filler, &edits);
        edits.extend(typos);
    }
    for (index, edit) in edits.iter_mut().enumerate() {
        edit.id = format!("e{}", index + 1);
    }
    edits
}

fn phrase_candidates(units: &[TextUnit], filler: &[FillerCandidate]) -> Vec<EditCandidate> {
    let mut edits = Vec::new();
    for (unit_index, unit) in units.iter().enumerate() {
        let protected = code_mask(&unit.text);
        for (range, replacement) in phrase_matches(&unit.text) {
            let blocked = edits.len() >= MAX_PHRASE_CANDIDATES
                || !word_boundary(&unit.text, &range)
                || overlaps(&range, unit_index, filler, &edits)
                || protected[range.clone()].iter().any(|value| *value);
            if !blocked {
                edits.push(phrase_edit(unit_index, &unit.text, range, replacement));
            }
        }
    }
    edits
}

fn phrase_matches(text: &str) -> Vec<(Range<usize>, &'static str)> {
    let lower = text.to_ascii_lowercase();
    PHRASES
        .iter()
        .flat_map(|(phrase, replacement)| {
            lower
                .match_indices(phrase)
                .map(|(start, _)| (start..start + phrase.len(), *replacement))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn phrase_edit(
    unit_index: usize,
    text: &str,
    range: Range<usize>,
    replacement: &str,
) -> EditCandidate {
    let original = text[range.clone()].to_owned();
    let replacement = if original.starts_with(char::is_uppercase) {
        capitalize(replacement)
    } else {
        replacement.to_owned()
    };
    EditCandidate {
        id: String::new(),
        kind: "phrasing",
        unit_index,
        range,
        original,
        choices: vec![replacement],
    }
}

/// Bounded Hunspell alternatives for plain lowercase words outside code and edits.
fn typo_candidates(
    units: &[TextUnit],
    filler: &[FillerCandidate],
    edits: &[EditCandidate],
) -> Vec<EditCandidate> {
    let locations = spell_locations(units, filler, edits);
    let words: BTreeSet<_> = locations.iter().map(|(_, _, word)| word.clone()).collect();
    let suggestions = hunspell_suggestions(&words.into_iter().collect::<Vec<_>>());
    locations
        .into_iter()
        .filter_map(|(unit_index, range, word)| {
            let choices = suggestions
                .get(&word)
                .filter(|choices| !choices.is_empty())?;
            Some(EditCandidate {
                id: String::new(),
                kind: "typo",
                unit_index,
                range,
                choices: choices.clone(),
                original: word,
            })
        })
        .take(MAX_TYPO_CANDIDATES)
        .collect()
}

fn spell_locations(
    units: &[TextUnit],
    filler: &[FillerCandidate],
    edits: &[EditCandidate],
) -> Vec<(usize, Range<usize>, String)> {
    let mut words = BTreeSet::new();
    let mut locations = Vec::new();
    for (unit_index, unit) in units.iter().enumerate() {
        let protected = code_mask(&unit.text);
        for range in alphabetic_runs(&unit.text) {
            let word = &unit.text[range.clone()];
            let eligible = words.len() < MAX_SPELL_WORDS
                && spellable(word)
                && word_boundary(&unit.text, &range)
                && !overlaps(&range, unit_index, filler, edits)
                && !protected[range.clone()].iter().any(|value| *value);
            if eligible {
                words.insert(word.to_owned());
                locations.push((unit_index, range, word.to_owned()));
            }
        }
    }
    locations
}

fn alphabetic_runs(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut runs = Vec::new();
    let mut start = None;
    for (index, byte) in bytes.iter().chain(std::iter::once(&b' ')).enumerate() {
        match (start, byte.is_ascii_alphabetic()) {
            (None, true) => start = Some(index),
            (Some(begin), false) => {
                runs.push(begin..index);
                start = None;
            }
            _ => {}
        }
    }
    runs
}

fn spellable(word: &str) -> bool {
    (3..=24).contains(&word.len())
        && word.bytes().all(|byte| byte.is_ascii_lowercase())
        && !TECHNICAL_WORDS.contains(&word)
}

fn hunspell_suggestions(words: &[String]) -> BTreeMap<String, Vec<String>> {
    if words.is_empty() {
        return BTreeMap::new();
    }
    let Ok(mut child) = Command::new("timeout")
        .args(["0.3s", "hunspell", "-a", "-d", "en_US"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return BTreeMap::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        for word in words {
            if writeln!(stdin, "{word}").is_err() {
                return BTreeMap::new();
            }
        }
    }
    let Ok(output) = child.wait_with_output() else {
        return BTreeMap::new();
    };
    if !output.status.success() {
        return BTreeMap::new();
    }
    parse_suggestions(&String::from_utf8_lossy(&output.stdout), words)
}

fn parse_suggestions(output: &str, words: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut result = BTreeMap::new();
    let mut lines = output.lines().skip(1).filter(|line| !line.is_empty());
    for word in words {
        let Some(line) = lines.next() else { break };
        if !line.starts_with('&') {
            continue;
        }
        let Some((_, raw)) = line.split_once(": ") else {
            continue;
        };
        let choices = raw
            .split(", ")
            .filter(|suggestion| {
                suggestion.bytes().all(|byte| byte.is_ascii_lowercase())
                    && *suggestion != word
                    && edit_distance_at_most_two(word, suggestion)
            })
            .take(3)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !choices.is_empty() {
            result.insert(word.clone(), choices);
        }
    }
    result
}

fn edit_distance_at_most_two(left: &str, right: &str) -> bool {
    if left.len().abs_diff(right.len()) > 2 {
        return false;
    }
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (row, left_byte) in left.bytes().enumerate() {
        current[0] = row + 1;
        for (column, right_byte) in right.bytes().enumerate() {
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(previous[column] + usize::from(left_byte != right_byte));
        }
        std::mem::swap(&mut current, &mut previous);
    }
    previous[right.len()] <= 2
}

fn word_boundary(text: &str, range: &Range<usize>) -> bool {
    let before = text[..range.start].chars().next_back();
    let after = text[range.end..].chars().next();
    !before.is_some_and(|value| value.is_alphanumeric() || "_'@/.-$".contains(value))
        && !after.is_some_and(|value| value.is_alphanumeric() || "_'@/.-".contains(value))
}

fn overlaps(
    range: &Range<usize>,
    unit_index: usize,
    filler: &[FillerCandidate],
    edits: &[EditCandidate],
) -> bool {
    filler
        .iter()
        .any(|candidate| candidate.unit_index == unit_index && intersects(range, &candidate.range))
        || edits.iter().any(|candidate| {
            candidate.unit_index == unit_index && intersects(range, &candidate.range)
        })
}

fn intersects(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}

#[cfg(test)]
mod tests {
    use typesafe_ai::Json;

    use super::*;
    use crate::segment::structured_units;

    #[test]
    fn parses_bounded_spelling_alternatives() {
        let words = vec!["contex".to_owned(), "teh".to_owned()];
        let output =
            "@(#) Hunspell\n& contex 3 0: context, cortex, content\n\n& teh 2 0: the, tech\n";
        let found = parse_suggestions(output, &words);
        assert_eq!(found["contex"][0], "context");
        assert_eq!(found["teh"][0], "the");
    }

    #[test]
    fn phrase_candidates_avoid_code_and_paths() {
        let units = structured_units(&Json::from(
            "In order to test, edit `in order to` but keep /in/order/to.",
        ));
        let found = candidates(
            &units,
            &[],
            &EnhancementOptions {
                rephrase: true,
                ..EnhancementOptions::default()
            },
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, "In order to");
        assert_eq!(found[0].choices, ["To"]);
    }
}
