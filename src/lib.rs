//! Low-latency, loss-aware JSON/TOON context standardization guided by Jev.

mod context;
mod decisions;
mod draft;
mod edits;
mod encoding;
mod guidance;
pub mod harness;
mod jev;
mod model;
mod questions;
mod render;
mod segment;
mod standardize;
mod target;
mod tokens;

pub use model::{
    Alternative, BackgroundItem, ContextDecision, DecisionSource, EditDecision, Encoding,
    EncodingDecision, EnhancementOptions, FillerDecision, FormatPreference, GuidanceDecision,
    RoleDecision, SegmentRole, SegmentationDecision, StandardizeError, StandardizeOptions,
    StandardizeResult, StandardizeStats,
};
pub use standardize::standardize;
pub use tokens::estimate_tokens;
