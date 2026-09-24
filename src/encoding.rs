use std::collections::BTreeMap;

use typesafe_ai::{Json, SystemOneResponse};

use crate::draft::Draft;
use crate::model::{
    Alternative, DecisionSource, Encoding, EncodingDecision, FormatPreference, Result,
    StandardizeError, StandardizeOptions,
};
use crate::render::{
    encode_csv, encode_json, encode_markdown, encode_plain, encode_toon, encode_xml, roles,
};
use crate::target::Target;
use crate::tokens::estimate_tokens;

/// Every encoding of one value; `None` where that encoding cannot represent it.
pub(crate) struct Candidates {
    pub json: String,
    pub toon: String,
    pub csv: Option<String>,
    pub xml: Option<String>,
    pub markdown: Option<String>,
    pub plain: Option<String>,
}

/// The formats a user can switch between in the preview.
const SWITCHABLE: [Encoding; 4] = [
    Encoding::Plain,
    Encoding::Xml,
    Encoding::Markdown,
    Encoding::Json,
];

impl Candidates {
    /// `plain` is the prose itself, when there is prose. CSV is withheld when a skill
    /// invocation must stay obvious in the prompt.
    pub(crate) fn of(value: &Json, plain: Option<&str>, allow_csv: bool) -> Result<Self> {
        Ok(Self {
            json: encode_json(value)?,
            toon: encode_toon(value)?,
            csv: if allow_csv { encode_csv(value) } else { None },
            xml: encode_xml(value),
            markdown: encode_markdown(value),
            plain: plain.and_then(|text| encode_plain(text, value)),
        })
    }

    pub(crate) fn get(&self, encoding: Encoding) -> Option<&str> {
        match encoding {
            Encoding::Json => Some(&self.json),
            Encoding::Toon => Some(&self.toon),
            Encoding::Csv => self.csv.as_deref(),
            Encoding::Xml => self.xml.as_deref(),
            Encoding::Markdown => self.markdown.as_deref(),
            Encoding::Plain => self.plain.as_deref(),
        }
    }

    fn alternatives(&self) -> Vec<Alternative> {
        SWITCHABLE
            .into_iter()
            .filter_map(|encoding| {
                let text = self.get(encoding)?.to_owned();
                Some(Alternative { encoding, text })
            })
            .collect()
    }
}

pub(crate) fn unavailable_reason(encoding: Encoding) -> Option<&'static str> {
    match encoding {
        Encoding::Csv => {
            Some("CSV requires a uniform table of scalar records without extra wrapper fields.")
        }
        Encoding::Xml => {
            Some("XML requires string fields or role/text segments without colliding closing tags.")
        }
        Encoding::Markdown => Some("Markdown requires string fields or role/text segments."),
        Encoding::Plain => Some("Plain text requires a prose draft."),
        Encoding::Json | Encoding::Toon => None,
    }
}

/// The exact downstream text and how its encoding was chosen.
pub(crate) struct Payload {
    pub encoding: Encoding,
    pub decision: EncodingDecision,
    pub text: String,
    pub alternatives: Vec<Alternative>,
}

pub(crate) fn encode(
    value: &Json,
    plain: Option<&str>,
    draft: &Draft<'_>,
    response: Option<&SystemOneResponse>,
    options: &StandardizeOptions,
) -> Result<Payload> {
    let candidates = Candidates::of(value, plain, allow_csv(draft, options))?;
    let target = Target {
        model: options.target_model.as_deref(),
        harness: options.harness(),
    };
    let prose = draft.prose.then_some((target, one_kind(value)));
    let decision = decide(&candidates, prose, response, options);
    let text = candidates
        .get(decision.encoding)
        .map(str::to_owned)
        .ok_or_else(|| unavailable(decision.encoding))?;
    Ok(Payload {
        encoding: decision.encoding,
        decision,
        text,
        alternatives: candidates.alternatives(),
    })
}

/// Anthropic recommends tags "especially when your prompt mixes instructions, context,
/// examples, and variable inputs"; a draft of one kind gains nothing from them.
pub(crate) fn one_kind(value: &Json) -> bool {
    let roles = roles(value);
    roles.windows(2).all(|pair| pair[0] == pair[1])
}

/// Fails before any network call when a forced format cannot represent the draft.
pub(crate) fn check_forced(draft: &Draft<'_>, options: &StandardizeOptions) -> Result<()> {
    let Some(encoding) = forced(options.format) else {
        return Ok(());
    };
    let prose = match draft.source {
        Json::String(text) if draft.prose => Some(text.as_str()),
        _ => None,
    };
    let candidates = Candidates::of(&draft.provisional, prose, true)?;
    match candidates.get(encoding) {
        Some(_) => Ok(()),
        None => Err(unavailable(encoding)),
    }
}

/// Jev only votes on the encoding of structured data; prose uses its target's format.
pub(crate) fn data_candidates(
    draft: &Draft<'_>,
    options: &StandardizeOptions,
) -> Result<Option<Candidates>> {
    if draft.prose || options.format != FormatPreference::Auto {
        return Ok(None);
    }
    Candidates::of(&draft.provisional, None, allow_csv(draft, options)).map(Some)
}

fn allow_csv(draft: &Draft<'_>, options: &StandardizeOptions) -> bool {
    options.format != FormatPreference::Auto || draft.skill_references.is_empty()
}

fn unavailable(encoding: Encoding) -> StandardizeError {
    StandardizeError::InvalidOption(unavailable_reason(encoding).unwrap_or_default().to_owned())
}

fn forced(format: FormatPreference) -> Option<Encoding> {
    match format {
        FormatPreference::Json => Some(Encoding::Json),
        FormatPreference::Toon => Some(Encoding::Toon),
        FormatPreference::Csv => Some(Encoding::Csv),
        FormatPreference::Xml => Some(Encoding::Xml),
        FormatPreference::Markdown => Some(Encoding::Markdown),
        FormatPreference::Plain => Some(Encoding::Plain),
        FormatPreference::Auto => None,
    }
}

fn decide(
    candidates: &Candidates,
    prose: Option<(Target<'_>, bool)>,
    response: Option<&SystemOneResponse>,
    options: &StandardizeOptions,
) -> EncodingDecision {
    if let Some(encoding) = forced(options.format) {
        let reason = format!("{} forced by the format option.", encoding.as_str());
        return fixed(encoding, DecisionSource::Forced, reason);
    }
    let Some((target, one_kind)) = prose else {
        return data_decision(candidates, response, options.min_confidence);
    };
    let preferred = if one_kind {
        Encoding::Plain
    } else {
        target.native()
    };
    let encoding = prose_encoding(candidates, preferred);
    let source = if target.family().is_some() {
        DecisionSource::Target
    } else {
        DecisionSource::Heuristic
    };
    fixed(encoding, source, target.reason(encoding))
}

fn fixed(encoding: Encoding, source: DecisionSource, reason: String) -> EncodingDecision {
    EncodingDecision {
        encoding,
        source,
        confidence: None,
        probabilities: BTreeMap::new(),
        reason,
    }
}

/// Prose keeps its preferred format; the other prose formats cover a tag collision.
fn prose_encoding(candidates: &Candidates, preferred: Encoding) -> Encoding {
    [
        preferred,
        Encoding::Xml,
        Encoding::Markdown,
        Encoding::Plain,
    ]
    .into_iter()
    .find(|encoding| candidates.get(*encoding).is_some())
    .unwrap_or(Encoding::Json)
}

fn data_decision(
    candidates: &Candidates,
    response: Option<&SystemOneResponse>,
    min_confidence: f64,
) -> EncodingDecision {
    let answer = response
        .and_then(|response| response.choice("encoding"))
        .filter(|answer| answer.confidence >= min_confidence);
    let offered = |name: &str| {
        [Encoding::Json, Encoding::Toon, Encoding::Csv, Encoding::Xml]
            .into_iter()
            .find(|encoding| encoding.as_str() == name && candidates.get(*encoding).is_some())
    };
    let selected = answer.and_then(|answer| {
        offered(&answer.choice).or_else(|| {
            answer
                .probabilities
                .iter()
                .filter_map(|(name, probability)| Some((offered(name)?, probability)))
                .max_by(|(_, left), (_, right)| left.total_cmp(right))
                .map(|(encoding, _)| encoding)
        })
    });
    let Some(encoding) = selected else {
        let reason =
            "Structured data without a confident Jev answer: fewest estimated tokens.".to_owned();
        return fixed(smallest(candidates), DecisionSource::Heuristic, reason);
    };
    EncodingDecision {
        encoding,
        source: DecisionSource::Jev,
        confidence: answer.map(|answer| answer.confidence),
        probabilities: answer.map_or_else(BTreeMap::new, |answer| answer.probabilities.clone()),
        reason: format!("Structured data: Jev chose {}.", encoding.as_str()),
    }
}

fn smallest(candidates: &Candidates) -> Encoding {
    [Encoding::Json, Encoding::Toon, Encoding::Csv]
        .into_iter()
        .filter_map(|encoding| Some((encoding, candidates.get(encoding)?)))
        .min_by_key(|(_, text)| estimate_tokens(text))
        .map_or(Encoding::Json, |(encoding, _)| encoding)
}
