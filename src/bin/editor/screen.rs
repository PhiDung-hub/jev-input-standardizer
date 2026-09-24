use std::io::{self, Write};

use jev_input_standardizer::{DecisionSource, Encoding, GuidanceDecision, StandardizeResult};

use super::paint::Paint;

const MAX_LINES: usize = 40;

/// Format keys on the review screen.
const FORMATS: [(u8, Encoding, &str); 4] = [
    (b'p', Encoding::Plain, "plain"),
    (b'x', Encoding::Xml, "XML"),
    (b'm', Encoding::Markdown, "Markdown"),
    (b'j', Encoding::Json, "JSON"),
];

pub(super) enum Action {
    Accept,
    /// Accept, then press Enter in the host pane.
    Send,
    Cancel,
    Show(Encoding),
    Ignore,
}

/// Header, the exact prompt in the shown format, a one-line summary, and the keys.
pub(super) fn draw(
    output: &mut impl Write,
    response: &StandardizeResult,
    shown: Encoding,
    paint: Paint,
    can_send: bool,
) -> io::Result<()> {
    writeln!(output, "{}", paint.rule(&header(response, shown)))?;
    let text = text_of(response, shown);
    let lines: Vec<_> = text.lines().collect();
    let mut in_notes = false;
    for line in lines.iter().take(MAX_LINES) {
        in_notes |= *line == "<standardizer_notes>";
        writeln!(output, "{}", paint.payload_line(line, in_notes))?;
        in_notes &= *line != "</standardizer_notes>";
    }
    if lines.len() > MAX_LINES {
        let more = format!(
            "… {} more lines go to the composer",
            lines.len() - MAX_LINES
        );
        writeln!(output, "{}", paint.dim(&more))?;
    }
    writeln!(output, "{}", jev_line(response, paint))?;
    write!(output, "{} ", keys(response, shown, paint, can_send))
}

pub(super) fn text_of(response: &StandardizeResult, shown: Encoding) -> &str {
    response
        .alternatives
        .iter()
        .find(|alternative| alternative.encoding == shown)
        .map_or(&response.text, |alternative| &alternative.text)
}

pub(super) fn action(key: u8, response: &StandardizeResult, can_send: bool) -> Action {
    match key {
        b'y' | b'Y' => Action::Accept,
        b's' | b'S' if can_send => Action::Send,
        b'n' | b'N' | b'\r' | b'\n' | 3 | 27 => Action::Cancel,
        key => FORMATS
            .iter()
            .find(|(format_key, encoding, _)| {
                *format_key == key.to_ascii_lowercase() && available(response, *encoding)
            })
            .map_or(Action::Ignore, |(_, encoding, _)| Action::Show(*encoding)),
    }
}

fn available(response: &StandardizeResult, encoding: Encoding) -> bool {
    response
        .alternatives
        .iter()
        .any(|alternative| alternative.encoding == encoding)
}

fn header(response: &StandardizeResult, shown: Encoding) -> String {
    let target = response
        .target_model
        .as_deref()
        .or(response.host.as_deref())
        .unwrap_or("the target model");
    let why = match (
        shown == response.encoding,
        response.encoding_decision.source,
        shown,
    ) {
        (false, _, _) => "your choice",
        (true, DecisionSource::Forced, _) => "your default",
        (true, _, Encoding::Plain) if mixed(response) => "mixed content, kept as typed",
        (true, _, Encoding::Plain) => "one kind of content",
        (true, _, Encoding::Xml | Encoding::Markdown) => "mixed content",
        _ => "fallback",
    };
    format!("{} for {target} · {why}", label(shown))
}

/// Parts of different kinds (Jev's role where confirmed, else the heuristic's) that went
/// out plain were kept as typed, not judged one kind of content.
fn mixed(response: &StandardizeResult) -> bool {
    let roles = &response.role_decisions;
    roles.windows(2).any(|pair| pair[0].role != pair[1].role)
}

/// Jev's judgments on one line: what applied collapses to its answer (✓); what Jev
/// answered below `min_confidence`, and so did not apply, shows that answer and its
/// confidence (?).
fn jev_line(response: &StandardizeResult, paint: Paint) -> String {
    // ponytail: u128 → f64 loses precision only past 2^53 ms.
    #[allow(clippy::cast_precision_loss)]
    let seconds = response.stats.elapsed_ms as f64 / 1_000.0;
    let mut tail = format!("{seconds:.1} s");
    if response.stats.jev_requests > 1 {
        tail.push_str(" · backup request");
    }
    if !response.stats.jev_called {
        return format!(
            "{} jev not asked · {tail} {}",
            paint.dim("──"),
            paint.dim("──")
        );
    }
    let applied = applied(response);
    let mut parts = vec![format!(
        "{} {}",
        paint.green("✓"),
        if applied.is_empty() {
            "no changes".to_owned()
        } else {
            applied.join(" · ")
        }
    )];
    let uncertain = uncertain(response);
    if !uncertain.is_empty() {
        parts.push(format!("{} {}", paint.yellow("?"), uncertain.join(" · ")));
    }
    format!(
        "{} jev {} · {tail} {}",
        paint.dim("──"),
        parts.join(&paint.dim(" │ ")),
        paint.dim("──")
    )
}

fn applied(response: &StandardizeResult) -> Vec<String> {
    let mut roles: Vec<(&str, usize)> = Vec::new();
    for decision in response.role_decisions.iter().filter(|role| role.applied) {
        match roles
            .iter_mut()
            .find(|(role, _)| *role == decision.role.as_str())
        {
            Some((_, count)) => *count += 1,
            None => roles.push((decision.role.as_str(), 1)),
        }
    }
    let roles = roles.into_iter().map(|(role, count)| match count {
        1 => role.to_owned(),
        count => format!("{role} ×{count}"),
    });
    let edits = response
        .edit_decisions
        .iter()
        .filter(|edit| edit.applied)
        .filter_map(|edit| Some(change(&edit.original, edit.replacement.as_deref()?)));
    let removed = response
        .filler_decisions
        .iter()
        .filter(|filler| filler.removed)
        .map(|filler| format!("−{}", filler.phrase));
    let context = response
        .context_decisions
        .iter()
        .filter(|decision| decision.included)
        .map(|decision| format!("{} context", decision.source));
    roles
        .chain(edits)
        .chain(removed)
        .chain(guidance(response, true))
        .chain(context)
        .collect()
}

/// `teh → the`, or `+?` for an insertion such as a closing question mark.
fn change(original: &str, replacement: &str) -> String {
    if original.is_empty() {
        format!("+{replacement}")
    } else {
        format!("{original} → {replacement}")
    }
}

const MAX_UNCERTAIN: usize = 4;

fn uncertain(response: &StandardizeResult) -> Vec<String> {
    let min = response.min_confidence;
    // In a draft of several parts an unconfirmed part goes out untagged, whatever the
    // fallback guessed, so every unsure answer there could change the prompt.
    let several = response.role_decisions.len() > 1;
    let roles = response
        .role_decisions
        .iter()
        .filter(|role| {
            !role.applied
                && role
                    .answer
                    .is_some_and(|answer| several || answer != role.role)
        })
        .filter_map(|role| {
            let answer = role.answer?.as_str();
            Some(format!("{} {answer} {:.2}", role.id, role.confidence?))
        });
    let splits = response
        .segmentation_decisions
        .iter()
        .filter(|split| split.source == DecisionSource::Heuristic)
        .filter_map(|split| {
            // Shown only when Jev leaned the other way from the layout kept.
            let probability = split
                .split_probability
                .filter(|value| (*value >= 0.5) != split.split)?;
            Some(format!(
                "{}|{} split {probability:.2}",
                split.before, split.after
            ))
        });
    let edits = response
        .edit_decisions
        .iter()
        .filter(|edit| !edit.applied && edit.confidence.is_some_and(|value| value >= 0.5))
        .filter_map(|edit| {
            let replacement = edit.replacement.as_deref()?;
            Some(format!(
                "{} {:.2}",
                change(&edit.original, replacement),
                edit.confidence?
            ))
        });
    let filler = response.filler_decisions.iter().filter_map(|filler| {
        let probability = filler.remove_probability?;
        (probability >= 0.5 && probability < min)
            .then(|| format!("−{} {probability:.2}", filler.phrase))
    });
    let context = response
        .context_decisions
        .iter()
        .filter(|decision| !decision.included)
        .filter_map(|decision| {
            let probability = decision.include_probability.filter(|value| *value >= 0.5)?;
            Some(format!("{} context {probability:.2}", decision.source))
        });
    let mut items: Vec<String> = roles
        .chain(splits)
        .chain(edits)
        .chain(filler)
        .chain(guidance(response, false))
        .chain(context)
        .collect();
    if items.len() > MAX_UNCERTAIN {
        let more = items.len() - MAX_UNCERTAIN;
        items.truncate(MAX_UNCERTAIN);
        items.push(format!("+{more} more"));
    }
    items
}

/// Answer-style and research judgments, applied or not, other than the neutral ones.
fn guidance(response: &StandardizeResult, applied: bool) -> Vec<String> {
    let label = |decision: &GuidanceDecision, neutral: &str, kind: &str| {
        let shown = decision.applied == applied && decision.choice != neutral;
        let confidence = decision.confidence.filter(|_| !applied);
        shown.then(|| match confidence {
            Some(confidence) => format!("{} {kind} {confidence:.2}", decision.choice),
            None => format!("{} {kind}", decision.choice),
        })
    };
    let answer = response
        .answer_style_decision
        .as_ref()
        .and_then(|decision| label(decision, "standard", "answer"));
    let research = response
        .research_decision
        .as_ref()
        .and_then(|decision| label(decision, "none", "research"));
    answer.into_iter().chain(research).collect()
}

fn keys(response: &StandardizeResult, shown: Encoding, paint: Paint, can_send: bool) -> String {
    let formats: Vec<_> = FORMATS
        .iter()
        .filter(|(_, encoding, _)| available(response, *encoding))
        .map(|(key, encoding, name)| {
            let option = format!("{} {name}", char::from(*key));
            if *encoding == shown {
                paint.chosen(&option)
            } else {
                option
            }
        })
        .collect();
    let send = if can_send { " s send ·" } else { "" };
    format!(
        "{} {}{} {}",
        paint.bold(&paint.yellow("Accept?")),
        paint.dim("y/n ·"),
        send,
        formats.join(&paint.dim(" · "))
    )
}

fn label(encoding: Encoding) -> &'static str {
    FORMATS
        .iter()
        .find(|(_, candidate, _)| *candidate == encoding)
        .map_or(encoding.as_str(), |(_, _, name)| name)
}
