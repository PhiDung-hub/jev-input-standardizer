pub(crate) const QUESTION_STARTS: [&str; 16] = [
    "how", "what", "why", "when", "where", "who", "which", "can", "could", "should", "would",
    "does", "do", "is", "are", "will",
];

pub(super) fn question_clause_after_comma(left: &str, right: &str) -> bool {
    if !left.ends_with(',') || !right.contains('?') {
        return false;
    }
    let mut words = right.split_whitespace();
    let first = words.next().unwrap_or_default();
    let first = first.trim_start_matches(|character: char| !character.is_ascii_alphabetic());
    let question_word = if matches!(
        first.to_ascii_lowercase().as_str(),
        "also" | "and" | "but" | "or"
    ) {
        words.next().unwrap_or_default()
    } else {
        first
    };
    let question_word =
        question_word.trim_end_matches(|character: char| !character.is_ascii_alphabetic());
    QUESTION_STARTS.contains(&question_word.to_ascii_lowercase().as_str())
}

pub(crate) fn looks_like_question(text: &str) -> bool {
    let text = text.trim();
    let first = text.split_whitespace().next().unwrap_or_default();
    let first = first.trim_start_matches(|character: char| !character.is_ascii_alphabetic());
    let first = first.trim_end_matches(|character: char| !character.is_ascii_alphabetic());
    text.ends_with('?') && QUESTION_STARTS.contains(&first.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_a_follow_up_question_but_not_another_task() {
        assert!(question_clause_after_comma(
            "show as 0,",
            "also how do we pin a tab now?"
        ));
        assert!(!question_clause_after_comma(
            "show as 4,",
            "show as 0, also how do we pin a tab now?"
        ));
        assert!(!question_clause_after_comma(
            "keep a list,",
            "and how to pin a tab"
        ));
    }
}
