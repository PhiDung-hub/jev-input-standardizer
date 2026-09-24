use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::harness::Harness;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatPreference {
    #[default]
    Auto,
    Json,
    Toon,
    Csv,
    Xml,
    Markdown,
    Plain,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Json,
    Toon,
    Csv,
    Xml,
    Markdown,
    Plain,
}

impl Encoding {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Toon => "toon",
            Self::Csv => "csv",
            Self::Xml => "xml",
            Self::Markdown => "markdown",
            Self::Plain => "plain",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentRole {
    Task,
    Question,
    Context,
    Constraint,
    Input,
    Output,
    Example,
}

impl SegmentRole {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "task" => Some(Self::Task),
            "question" => Some(Self::Question),
            "context" => Some(Self::Context),
            "constraint" => Some(Self::Constraint),
            "input" => Some(Self::Input),
            "output" => Some(Self::Output),
            "example" => Some(Self::Example),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Question => "question",
            Self::Context => "context",
            Self::Constraint => "constraint",
            Self::Input => "input",
            Self::Output => "output",
            Self::Example => "example",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StandardizeOptions {
    /// Harness id or name from [`crate::harness::HARNESSES`]; unknown values act as none.
    pub host: Option<String>,
    /// Model the host session sends the payload to, such as `claude-opus-5-5`.
    pub target_model: Option<String>,
    /// Background for Jev, oldest first; items it says the draft depends on are quoted
    /// into the payload's notes.
    pub background: Vec<BackgroundItem>,
    pub format: FormatPreference,
    pub remove_filler: bool,
    pub restructure: bool,
    pub enhancements: EnhancementOptions,
    pub jev_min_chars: usize,
    pub max_segments: usize,
    pub max_filler_candidates: usize,
    /// A Jev judgment applies only at this confidence: a Choice's top answer at or
    /// above it, a Noul at or above it (yes) or at or below `1 - it` (no). Anything
    /// less falls back to the user's own layout and wording.
    pub min_confidence: f64,
    pub max_input_chars: usize,
    pub request_timeout_ms: u64,
    /// Send one identical backup request when Jev has not answered by then; the first
    /// answer wins. `0` disables it.
    pub hedge_after_ms: u64,
    pub model: Option<String>,
}

impl StandardizeOptions {
    #[must_use]
    pub fn harness(&self) -> Option<&'static Harness> {
        self.host.as_deref().and_then(crate::harness::find)
    }
}

impl Default for StandardizeOptions {
    fn default() -> Self {
        Self {
            host: None,
            target_model: None,
            background: Vec::new(),
            format: FormatPreference::Auto,
            remove_filler: true,
            restructure: true,
            enhancements: EnhancementOptions::default(),
            jev_min_chars: 160,
            max_segments: 32,
            max_filler_candidates: 32,
            min_confidence: 0.8,
            max_input_chars: 64 * 1024,
            request_timeout_ms: 2_000,
            hedge_after_ms: 700,
            model: None,
        }
    }
}

/// Material Jev may quote to resolve references in the draft: earlier conversation
/// turns (`source` "user" or "assistant") or anything a context hook supplies.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BackgroundItem {
    pub source: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EnhancementOptions {
    pub correct_typos: bool,
    pub rephrase: bool,
    pub response_guidance: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    Jev,
    Heuristic,
    Forced,
    /// The target model's native prompt format.
    Target,
}

impl DecisionSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::Heuristic => "heuristic",
            Self::Forced => "forced",
            Self::Target => "target",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodingDecision {
    pub encoding: Encoding,
    pub source: DecisionSource,
    pub confidence: Option<f64>,
    pub probabilities: BTreeMap<String, f64>,
    /// Why this encoding was chosen, for the preview.
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextDecision {
    pub id: String,
    pub source: String,
    pub include_probability: Option<f64>,
    pub included: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FillerDecision {
    pub id: String,
    pub path: String,
    pub phrase: String,
    pub remove_probability: Option<f64>,
    pub removed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDecision {
    pub id: String,
    /// The role used: Jev's answer when confident, else the heuristic's.
    pub role: SegmentRole,
    /// Jev's top answer, applied or not.
    pub answer: Option<SegmentRole>,
    pub confidence: Option<f64>,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentationDecision {
    pub id: String,
    pub before: String,
    pub after: String,
    pub split_probability: Option<f64>,
    pub split: bool,
    pub source: DecisionSource,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditDecision {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub original: String,
    pub replacement: Option<String>,
    pub applied: bool,
    pub confidence: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuidanceDecision {
    /// Jev's top answer, or the neutral default when Jev did not answer.
    pub choice: String,
    pub confidence: Option<f64>,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardizeStats {
    pub input_chars: usize,
    pub output_chars: usize,
    pub input_tokens_estimate: usize,
    pub output_tokens_estimate: usize,
    pub background_items: usize,
    pub segments: usize,
    pub segmentation_candidates: usize,
    pub filler_candidates: usize,
    pub filler_removed: usize,
    pub edits_applied: usize,
    pub jev_called: bool,
    pub jev_requests: usize,
    pub jev_input_tokens: u64,
    pub elapsed_ms: u128,
}

/// The same draft in another format, so the preview can switch without a new request.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Alternative {
    pub encoding: Encoding,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardizeResult {
    pub encoding: Encoding,
    pub host: Option<String>,
    pub target_model: Option<String>,
    pub skill_references: Vec<String>,
    pub text: String,
    /// The confidence a judgment needed to apply.
    pub min_confidence: f64,
    /// Plain, XML, Markdown, and JSON renderings that can represent this draft.
    pub alternatives: Vec<Alternative>,
    pub encoding_decision: EncodingDecision,
    pub segmentation_decisions: Vec<SegmentationDecision>,
    pub filler_decisions: Vec<FillerDecision>,
    pub edit_decisions: Vec<EditDecision>,
    pub role_decisions: Vec<RoleDecision>,
    pub answer_style_decision: Option<GuidanceDecision>,
    pub research_decision: Option<GuidanceDecision>,
    pub context_decisions: Vec<ContextDecision>,
    pub stats: StandardizeStats,
}

#[derive(Debug, thiserror::Error)]
pub enum StandardizeError {
    #[error("invalid standardization option: {0}")]
    InvalidOption(String),
    #[error("context is {actual} characters; maximum is {maximum}")]
    InputTooLarge { actual: usize, maximum: usize },
    #[error("TOON encoding failed: {0}")]
    Toon(String),
    #[error(transparent)]
    TypeSafe(#[from] typesafe_ai::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub(crate) type Result<T> = std::result::Result<T, StandardizeError>;
