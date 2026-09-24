use typesafe_ai::Json;

use crate::decisions::provisional_context;
use crate::edits::{self, EditCandidate};
use crate::model::{Result, StandardizeError, StandardizeOptions};
use crate::render::encode_json;
use crate::segment::{
    BoundaryCandidate, FillerCandidate, TextUnit, filler_candidates, root_units, skill_references,
    structured_units,
};

/// The user's input split into text units, with every candidate change Jev may judge.
/// Built once and shared by the Jev request, the decisions, and the encoder.
pub(crate) struct Draft<'a> {
    pub source: &'a Json,
    pub input_text: String,
    pub input_chars: usize,
    /// A plain-text prompt that is segmented, rather than structured JSON input.
    pub prose: bool,
    pub units: Vec<TextUnit>,
    pub boundaries: Vec<BoundaryCandidate>,
    pub filler: Vec<FillerCandidate>,
    pub edits: Vec<EditCandidate>,
    pub skill_references: Vec<String>,
    /// The draft rebuilt with heuristic roles: Jev's view of structured input.
    pub provisional: Json,
}

impl<'a> Draft<'a> {
    pub(crate) fn prepare(source: &'a Json, options: &StandardizeOptions) -> Result<Self> {
        let input_text = encode_json(source)?;
        let input_chars = match source {
            Json::String(text) => text.len(),
            _ => input_text.len(),
        };
        if input_chars > options.max_input_chars {
            return Err(StandardizeError::InputTooLarge {
                actual: input_chars,
                maximum: options.max_input_chars,
            });
        }
        let prose = matches!(source, Json::String(_)) && options.restructure;
        let (units, boundaries) = match source {
            Json::String(text) if prose => root_units(text, options.max_segments),
            _ => (structured_units(source), Vec::new()),
        };
        let skill_references = options
            .harness()
            .map_or_else(Vec::new, |harness| skill_references(&units, harness.skills));
        let filler = if options.remove_filler {
            filler_candidates(&units, options.max_filler_candidates)
        } else {
            Vec::new()
        };
        let edits = edits::candidates(&units, &filler, &options.enhancements);
        let provisional = provisional_context(source, &units, prose);
        Ok(Self {
            source,
            input_text,
            input_chars,
            prose,
            units,
            boundaries,
            filler,
            edits,
            skill_references,
            provisional,
        })
    }
}
