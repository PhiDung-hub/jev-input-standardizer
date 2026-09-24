use std::ops::Range;

use crate::segment::{FillerCandidate, QUESTION_STARTS, TextUnit, code_mask};

use super::{EditCandidate, overlaps, word_boundary};

const MAX_QUESTION_CANDIDATES: usize = 4;

/// A proposed question edit: the range, its original text, and the replacement.
type Proposal = (Range<usize>, String, String);

/// Casing, "i" → "I", and a closing "?" for units that open with a question word.
pub(super) fn candidates(
    units: &[TextUnit],
    filler: &[FillerCandidate],
    existing: &[EditCandidate],
) -> Vec<EditCandidate> {
    let mut found: Vec<EditCandidate> = Vec::new();
    for (unit_index, unit) in units.iter().enumerate() {
        let Some(first_word) = question_word(&unit.text) else {
            continue;
        };
        let protected = code_mask(&unit.text);
        let blocked = |range: &Range<usize>| overlaps(range, unit_index, filler, existing);
        let proposals = [
            casing(&unit.text, first_word, &protected),
            pronoun(&unit.text, &protected, &blocked),
            punctuation(&unit.text),
        ];
        for (range, original, choice) in proposals.into_iter().flatten() {
            let free = found.len() < MAX_QUESTION_CANDIDATES
                && !blocked(&range)
                && !found
                    .iter()
                    .any(|edit| edit.unit_index == unit_index && edit.range == range);
            if free {
                found.push(EditCandidate {
                    id: String::new(),
                    kind: "question",
                    unit_index,
                    range,
                    original,
                    choices: vec![choice],
                });
            }
        }
    }
    found
}

fn question_word(text: &str) -> Option<&str> {
    let first = text.split_whitespace().next()?;
    let first = first.trim_end_matches(|character: char| !character.is_ascii_alphabetic());
    QUESTION_STARTS
        .contains(&first.to_ascii_lowercase().as_str())
        .then_some(first)
}

fn casing(text: &str, first_word: &str, protected: &[bool]) -> Option<Proposal> {
    let start = text.len() - text.trim_start().len();
    let lowercase = first_word.bytes().next()?.is_ascii_lowercase();
    (lowercase && !protected[start]).then(|| {
        let first = &first_word[..1];
        (
            start..start + 1,
            first.to_owned(),
            first.to_ascii_uppercase(),
        )
    })
}

fn pronoun(
    text: &str,
    protected: &[bool],
    blocked: &dyn Fn(&Range<usize>) -> bool,
) -> Option<Proposal> {
    let start = text
        .match_indices('i')
        .map(|(start, _)| start)
        .find(|&start| {
            let range = start..start + 1;
            let previous = text[..start]
                .split_whitespace()
                .next_back()
                .unwrap_or_default()
                .to_ascii_lowercase();
            word_boundary(text, &range)
                && matches!(
                    previous.as_str(),
                    "can" | "could" | "should" | "would" | "do" | "did" | "will" | "may"
                )
                && !protected[start]
                && !blocked(&range)
        })?;
    Some((start..start + 1, "i".to_owned(), "I".to_owned()))
}

fn punctuation(text: &str) -> Option<Proposal> {
    let trimmed = text.trim();
    let several_sentences = [". ", "! ", "\n", "?"]
        .iter()
        .any(|marker| trimmed.contains(marker));
    if several_sentences || imperative_do(trimmed) {
        return None;
    }
    let end = text.trim_end().len();
    let range = if trimmed.ends_with('.') {
        end - 1..end
    } else {
        end..end
    };
    Some((range.clone(), text[range].to_owned(), "?".to_owned()))
}

/// "Do not break it." and "Do the refactor." are commands; only "Do we …" or
/// "Does it …" with a subject asks something, so only those may gain a "?".
fn imperative_do(text: &str) -> bool {
    let mut words = text
        .split_whitespace()
        .map(|word| word.trim_matches(|character: char| !character.is_ascii_alphabetic()));
    let first = words.next().unwrap_or_default().to_ascii_lowercase();
    let second = words.next().unwrap_or_default().to_ascii_lowercase();
    matches!(first.as_str(), "do" | "does" | "did")
        && !matches!(
            second.as_str(),
            "i" | "you" | "we" | "they" | "he" | "she" | "it" | "this" | "that" | "these" | "those"
        )
}

#[cfg(test)]
mod tests {
    use typesafe_ai::Json;

    use super::*;
    use crate::segment::structured_units;

    #[test]
    fn proposes_casing_and_punctuation_without_rewriting_question() {
        let units = structured_units(&Json::from("how can i test this."));
        let found = candidates(&units, &[], &[]);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].choices, ["H"]);
        assert_eq!(found[1].choices, ["I"]);
        assert_eq!(found[2].choices, ["?"]);
    }

    #[test]
    fn leaves_existing_question_and_inline_code_alone() {
        let units = structured_units(&Json::from("What does `how` mean?"));
        assert!(candidates(&units, &[], &[]).is_empty());
    }

    #[test]
    fn imperative_do_keeps_its_period() {
        for text in ["Do not break the dashboard layout.", "Do the refactor."] {
            let units = structured_units(&Json::from(text));
            assert!(
                candidates(&units, &[], &[])
                    .iter()
                    .all(|edit| edit.choices != ["?"]),
                "{text}"
            );
        }
        let units = structured_units(&Json::from("do we need tests."));
        assert!(
            candidates(&units, &[], &[])
                .iter()
                .any(|edit| edit.choices == ["?"])
        );
    }

    #[test]
    fn does_not_change_the_final_period_of_multiple_sentences() {
        let units = structured_units(&Json::from("How does it work. Explain the implementation."));
        assert!(candidates(&units, &[], &[]).is_empty());
    }
}
