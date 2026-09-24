use typesafe_ai::{Json, SystemOneResponse};

use crate::guidance::add_note;
use crate::model::{BackgroundItem, ContextDecision};

// One comparative Choice discriminates far better than a Noul per item, which scored
// nearly everything 0.55-0.8 in live use; quoting needs `min_confidence`.
const MAX_INCLUDED: usize = 2;
const QUOTE_EDGE_CHARS: usize = 300;

pub(crate) fn item_id(index: usize) -> String {
    format!("b{}", index + 1)
}

/// Background the harness cannot know, with its ids. Conversation turns are already in
/// the harness's context, so Jev reads them but they are never quoted.
pub(crate) fn quotable(items: &[BackgroundItem]) -> Vec<(String, &BackgroundItem)> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| !matches!(item.source.as_str(), "user" | "assistant"))
        .map(|(index, item)| (item_id(index), item))
        .collect()
}

/// Quotes the outside material Jev says the draft depends on, oldest first, as a note.
pub(crate) fn apply(
    value: &mut Json,
    response: Option<&SystemOneResponse>,
    items: &[BackgroundItem],
    min_confidence: f64,
) -> Vec<ContextDecision> {
    let answer = response.and_then(|response| response.choice("context"));
    let candidates = quotable(items);
    let mut decisions: Vec<_> = candidates
        .iter()
        .map(|(id, item)| ContextDecision {
            id: id.clone(),
            source: item.source.clone(),
            include_probability: answer.and_then(|answer| answer.probabilities.get(id).copied()),
            included: false,
        })
        .collect();
    let probability = |index: usize| decisions[index].include_probability.unwrap_or(0.0);
    let mut selected: Vec<usize> = (0..decisions.len())
        .filter(|&index| probability(index) >= min_confidence)
        .collect();
    selected.sort_by(|&left, &right| probability(right).total_cmp(&probability(left)));
    selected.truncate(MAX_INCLUDED);
    selected.sort_unstable();
    if selected.is_empty() {
        return decisions;
    }
    let quoted = selected
        .iter()
        .map(|&index| {
            let item = candidates[index].1;
            format!("[{}] {}", item.source, quote(&item.text))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    add_note(value, "earlier_context", quoted);
    for index in selected {
        decisions[index].included = true;
    }
    decisions
}

/// The quote points the model at the turn; it already has the full text unless the
/// host compacted it, so both ends are enough. Closing tags of the notes block are
/// escaped so a quoted turn cannot knock the payload out of its target format.
fn quote(text: &str) -> String {
    let count = text.chars().count();
    let clipped = if count <= 2 * QUOTE_EDGE_CHARS {
        text.to_owned()
    } else {
        let head: String = text.chars().take(QUOTE_EDGE_CHARS).collect();
        let tail: String = text.chars().skip(count - QUOTE_EDGE_CHARS).collect();
        format!("{head} … {tail}")
    };
    clipped
        .replace("</earlier_context>", "&lt;/earlier_context>")
        .replace("</standardizer_notes>", "&lt;/standardizer_notes>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_keep_both_ends_on_char_boundaries() {
        let text = format!("{}{}", "é".repeat(400), "漢".repeat(400));
        let expected = format!("{} … {}", "é".repeat(300), "漢".repeat(300));
        assert_eq!(quote(&text), expected);
        assert_eq!(quote("short"), "short");
    }
}
