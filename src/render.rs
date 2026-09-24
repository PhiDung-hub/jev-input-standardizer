use std::fmt::Write;

use typesafe_ai::Json;

use crate::guidance::NOTES_KEY;
use crate::model::{Result, StandardizeError};

pub(crate) fn encode_json(value: &Json) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

pub(crate) fn encode_toon(value: &Json) -> Result<String> {
    toon_format::encode_default(value).map_err(|error| StandardizeError::Toon(error.to_string()))
}

pub(crate) fn encode_csv(value: &Json) -> Option<String> {
    let rows = match value {
        Json::Array(rows) => rows,
        Json::Object(values) if values.len() == 1 => match values.get("segments")? {
            Json::Array(rows) => rows,
            _ => return None,
        },
        _ => return None,
    };
    let first = match rows.first()? {
        Json::Object(row) if !row.is_empty() => row,
        _ => return None,
    };
    let headers = first.keys().cloned().collect::<Vec<_>>();
    let mut output = headers
        .iter()
        .map(|header| csv_field(header))
        .collect::<Vec<_>>()
        .join(",");
    output.push('\n');
    for row in rows {
        let Json::Object(values) = row else {
            return None;
        };
        if values.len() != headers.len()
            || !headers.iter().all(|header| values.contains_key(header))
        {
            return None;
        }
        let fields = headers
            .iter()
            .map(|header| scalar_text(&values[header]).map(|value| csv_field(&value)))
            .collect::<Option<Vec<_>>>()?;
        output.push_str(&fields.join(","));
        output.push('\n');
    }
    Some(output)
}

/// Renders prose payloads as XML-tagged sections, the delimiter style both major model
/// vendors' prompt guides recommend. Text stays verbatim; standardizer notes come last.
pub(crate) fn encode_xml(value: &Json) -> Option<String> {
    let Json::Object(fields) = value else {
        return None;
    };
    let mut output = String::new();
    let ordered = fields
        .iter()
        .filter(|(key, _)| *key != NOTES_KEY)
        .chain(fields.get_key_value(NOTES_KEY));
    for (key, entry) in ordered {
        match entry {
            Json::String(text) => push_element(&mut output, section_name(key), text)?,
            Json::Array(items) if key == "segments" => push_segments(&mut output, items)?,
            Json::Object(notes) => push_group(&mut output, key, notes)?,
            _ => return None,
        }
    }
    Some(output.trim_end().to_owned())
}

/// Renders prose payloads as Markdown sections in the user's order, with standardizer
/// notes in a trailing XML block, the shape Codex uses for GPT instructions and context.
pub(crate) fn encode_markdown(value: &Json) -> Option<String> {
    let Json::Object(fields) = value else {
        return None;
    };
    let mut sections = Vec::new();
    for (key, entry) in fields.iter().filter(|(key, _)| *key != NOTES_KEY) {
        match entry {
            Json::String(text) => sections.push(section(section_name(key), text)),
            Json::Array(items) if key == "segments" => sections.extend(
                grouped(items)?
                    .into_iter()
                    .enumerate()
                    .map(|(index, group)| markdown_section(index, group)),
            ),
            _ => return None,
        }
    }
    match fields.get(NOTES_KEY) {
        Some(Json::Object(notes)) => {
            let mut block = String::new();
            push_group(&mut block, NOTES_KEY, notes)?;
            sections.push(block.trim_end().to_owned());
        }
        Some(_) => return None,
        None => {}
    }
    Some(sections.join("\n\n"))
}

/// The user's own text (approved edits, original layout), then any notes as an XML block.
pub(crate) fn encode_plain(text: &str, value: &Json) -> Option<String> {
    let Json::Object(fields) = value else {
        return Some(text.to_owned());
    };
    match fields.get(NOTES_KEY) {
        Some(Json::Object(notes)) => {
            let mut block = String::new();
            push_group(&mut block, NOTES_KEY, notes)?;
            Some(format!("{text}\n\n{}", block.trim_end()))
        }
        Some(_) => None,
        None => Some(text.to_owned()),
    }
}

/// The roles in a prose payload, in order: segment roles, or the single role key.
pub(crate) fn roles(value: &Json) -> Vec<&str> {
    let Json::Object(fields) = value else {
        return Vec::new();
    };
    fields
        .iter()
        .filter(|(key, _)| *key != NOTES_KEY)
        .flat_map(|(key, entry)| match entry {
            Json::Array(items) if key == "segments" => items
                .iter()
                .filter_map(|item| role_text(item).map(|(role, _)| role))
                .filter(|role| !role.is_empty())
                .collect(),
            _ => vec![key.as_str()],
        })
        .collect()
}

/// Untagged text after a section follows a rule, so it does not read as part of it;
/// sections are joined by a blank line, so the rule never underlines a heading.
fn markdown_section(index: usize, (name, text): (&str, String)) -> String {
    match (name, index) {
        ("", 0) => text,
        ("", _) => format!("---\n{text}"),
        _ => section(name, &text),
    }
}

fn section(key: &str, text: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    let heading: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    format!("## {heading}\n{text}")
}

/// Section names from Anthropic's guide (`<instructions>`, `<context>`, `<input>`,
/// `<examples>`), which also match the GPT guide's prompt sections; plural because
/// neighbouring segments of one role share a section.
fn section_name(role: &str) -> &str {
    match role {
        "task" => "instructions",
        "constraint" => "constraints",
        "question" => "questions",
        "output" => "output_format",
        "example" => "examples",
        other => other,
    }
}

/// Consecutive segments of one role joined into one section, in the user's order.
fn grouped(items: &[Json]) -> Option<Vec<(&str, String)>> {
    let mut groups: Vec<(&str, String)> = Vec::new();
    for item in items {
        let (role, text) = role_text(item)?;
        let name = section_name(role);
        match groups.last_mut() {
            Some((last, body)) if *last == name => {
                body.push('\n');
                body.push_str(text);
            }
            _ => groups.push((name, text.to_owned())),
        }
    }
    Some(groups)
}

fn role_text(item: &Json) -> Option<(&str, &str)> {
    let Json::Object(segment) = item else {
        return None;
    };
    match (segment.get("role"), segment.get("text")) {
        (Some(Json::String(role)), Some(Json::String(text))) => Some((role, text)),
        // Untagged: the user's text where Jev confirmed no role.
        (Some(Json::Null), Some(Json::String(text))) => Some(("", text)),
        _ => None,
    }
}

fn push_segments(output: &mut String, items: &[Json]) -> Option<()> {
    for (name, text) in grouped(items)? {
        if name.is_empty() {
            let _ = writeln!(output, "{text}");
        } else {
            push_element(output, name, &text)?;
        }
    }
    Some(())
}

fn push_group(
    output: &mut String,
    tag: &str,
    fields: &std::collections::BTreeMap<String, Json>,
) -> Option<()> {
    let mut body = String::new();
    for (key, entry) in fields {
        let Json::String(text) = entry else {
            return None;
        };
        push_element(&mut body, key, text)?;
    }
    usable(tag, &body)?;
    let _ = writeln!(output, "<{tag}>\n{body}</{tag}>");
    Some(())
}

fn push_element(output: &mut String, tag: &str, text: &str) -> Option<()> {
    usable(tag, text)?;
    let separator = if text.contains('\n') { "\n" } else { "" };
    let _ = writeln!(output, "<{tag}>{separator}{text}{separator}</{tag}>");
    Some(())
}

fn usable(tag: &str, text: &str) -> Option<()> {
    let valid_tag = !tag.is_empty()
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_');
    (valid_tag && !text.contains(&format!("</{tag}>"))).then_some(())
}

fn scalar_text(value: &Json) -> Option<String> {
    match value {
        Json::Null => Some(String::new()),
        Json::Bool(value) => Some(value.to_string()),
        Json::I64(value) => Some(value.to_string()),
        Json::U64(value) => Some(value.to_string()),
        Json::F64(value) => Some(value.to_string()),
        Json::String(value) => Some(value.clone()),
        Json::Array(_) | Json::Object(_) => None,
    }
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn csv_requires_uniform_scalar_records_and_escapes_cells() {
        let rows = Json::Array(vec![
            Json::Object(BTreeMap::from([
                ("role".to_owned(), Json::from("task")),
                ("text".to_owned(), Json::from("build, test")),
            ])),
            Json::Object(BTreeMap::from([
                ("role".to_owned(), Json::from("constraint")),
                ("text".to_owned(), Json::from("say \"done\"")),
            ])),
        ]);

        assert_eq!(
            encode_csv(&rows).as_deref(),
            Some("role,text\ntask,\"build, test\"\nconstraint,\"say \"\"done\"\"\"\n")
        );
        assert!(encode_csv(&Json::from("not tabular")).is_none());
    }

    fn segments(items: &[(&str, &str)]) -> Json {
        Json::Array(
            items
                .iter()
                .map(|(role, text)| {
                    let role = if role.is_empty() {
                        Json::Null
                    } else {
                        Json::from(*role)
                    };
                    Json::Object(BTreeMap::from([
                        ("role".to_owned(), role),
                        ("text".to_owned(), Json::from(*text)),
                    ]))
                })
                .collect(),
        )
    }

    #[test]
    fn xml_keeps_segment_order_and_puts_notes_last() {
        let value = Json::Object(BTreeMap::from([
            (
                NOTES_KEY.to_owned(),
                Json::Object(BTreeMap::from([(
                    "research".to_owned(),
                    Json::from("Check the docs."),
                )])),
            ),
            (
                "segments".to_owned(),
                segments(&[
                    ("task", "Fix `a < b && c`."),
                    ("context", "line one\nline two"),
                    ("task", "Then ship."),
                ]),
            ),
        ]));

        assert_eq!(
            encode_xml(&value).unwrap(),
            "<instructions>Fix `a < b && c`.</instructions>\n<context>\nline one\nline two\n</context>\n<instructions>Then ship.</instructions>\n<standardizer_notes>\n<research>Check the docs.</research>\n</standardizer_notes>"
        );
    }

    #[test]
    fn neighbours_with_one_role_share_a_section_in_order() {
        let value = Json::Object(BTreeMap::from([(
            "segments".to_owned(),
            segments(&[
                ("task", "Fix the parser."),
                ("constraint", "Keep the API."),
                ("constraint", "Do not add dependencies."),
                ("question", "How do I test it?"),
            ]),
        )]));

        assert_eq!(
            encode_xml(&value).unwrap(),
            "<instructions>Fix the parser.</instructions>\n<constraints>\nKeep the API.\nDo not add dependencies.\n</constraints>\n<questions>How do I test it?</questions>"
        );
        assert_eq!(
            encode_markdown(&value).unwrap(),
            "## Instructions\nFix the parser.\n\n## Constraints\nKeep the API.\nDo not add dependencies.\n\n## Questions\nHow do I test it?"
        );
    }

    #[test]
    fn unconfirmed_segments_stay_untagged_and_are_no_kind() {
        let value = Json::Object(BTreeMap::from([(
            "segments".to_owned(),
            segments(&[
                ("", "If it breaks,"),
                ("task", "Fix it."),
                ("", "I will check back."),
                ("question", "Why?"),
            ]),
        )]));

        assert_eq!(
            encode_xml(&value).unwrap(),
            "If it breaks,\n<instructions>Fix it.</instructions>\nI will check back.\n<questions>Why?</questions>"
        );
        assert_eq!(
            encode_markdown(&value).unwrap(),
            "If it breaks,\n\n## Instructions\nFix it.\n\n---\nI will check back.\n\n## Questions\nWhy?"
        );
        assert_eq!(roles(&value), ["task", "question"]);
    }

    #[test]
    fn markdown_keeps_order_and_code_verbatim_with_notes_last() {
        let value = Json::Object(BTreeMap::from([
            (
                NOTES_KEY.to_owned(),
                Json::Object(BTreeMap::from([(
                    "earlier_context".to_owned(),
                    Json::from("[assistant] Two fixes remain."),
                )])),
            ),
            (
                "segments".to_owned(),
                segments(&[
                    ("task", "Do both."),
                    ("input", "```rust\nlet a = \"x\";\n```"),
                ]),
            ),
        ]));

        assert_eq!(
            encode_markdown(&value).unwrap(),
            "## Instructions\nDo both.\n\n## Input\n```rust\nlet a = \"x\";\n```\n\n<standardizer_notes>\n<earlier_context>[assistant] Two fixes remain.</earlier_context>\n</standardizer_notes>"
        );
        let single = Json::Object(BTreeMap::from([(
            "answer_style".to_owned(),
            Json::from("Be brief."),
        )]));
        assert_eq!(
            encode_markdown(&single).unwrap(),
            "## Answer style\nBe brief."
        );
        let numbers = Json::Object(BTreeMap::from([("retries".to_owned(), Json::from(3_u32))]));
        assert!(encode_markdown(&numbers).is_none());
    }

    #[test]
    fn xml_is_unavailable_for_colliding_tags_and_non_prose_values() {
        let collision = Json::Object(BTreeMap::from([(
            "task".to_owned(),
            Json::from("Close it with </instructions> please"),
        )]));
        let numbers = Json::Object(BTreeMap::from([("retries".to_owned(), Json::from(3_u32))]));
        let bad_tag = Json::Object(BTreeMap::from([("a b".to_owned(), Json::from("x"))]));

        assert!(encode_xml(&collision).is_none());
        assert!(encode_xml(&numbers).is_none());
        assert!(encode_xml(&bad_tag).is_none());
        assert!(encode_xml(&Json::from("bare")).is_none());
    }
}
