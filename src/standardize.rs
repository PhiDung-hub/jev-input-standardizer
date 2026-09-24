use std::time::Instant;

use typesafe_ai::{Client, Json, SystemOneResponse};

use crate::decisions::{self, Decided};
use crate::draft::Draft;
use crate::encoding::{self, Payload};
use crate::model::{
    ContextDecision, GuidanceDecision, Result, StandardizeError, StandardizeOptions,
    StandardizeResult, StandardizeStats,
};
use crate::tokens::estimate_tokens;
use crate::{context, guidance, jev, questions};

/// Standardize JSON-shaped context and return the exact downstream text plus a UI preview.
///
/// Jev supplies only bounded judgments in one fan-out request: semantic boundaries,
/// segment roles, safe filler deletions and edits, response guidance, the earlier
/// turn the draft depends on, and (for structured data) the encoding. Code owns every
/// transformation and preserves text when a judgment is unavailable.
///
/// # Errors
///
/// Returns option-validation, encoding, or `TypeSafe` transport/API errors.
pub async fn standardize(
    client: &Client,
    context: &Json,
    options: &StandardizeOptions,
) -> Result<StandardizeResult> {
    validate_options(options)?;
    let started = Instant::now();
    let draft = Draft::prepare(context, options)?;
    encoding::check_forced(&draft, options)?;
    let judgment = jev::judge(client, &draft, options).await?;
    let requests = judgment.as_ref().map_or(0, |judgment| judgment.requests);
    let response = judgment.as_ref().map(|judgment| &judgment.response);
    let mut applied = decisions::apply(&draft, response, options);
    let (answer_style_decision, research_decision, context_decisions) =
        annotate(&mut applied.standardized, &draft, response, options);
    let plain = applied.plain.as_deref();
    let payload = encoding::encode(&applied.standardized, plain, &draft, response, options)?;
    let stats = stats(
        &draft,
        &applied,
        &payload,
        (response, requests),
        options,
        started,
    );
    Ok(StandardizeResult {
        encoding: payload.encoding,
        host: options.host.clone(),
        target_model: options.target_model.clone(),
        skill_references: draft.skill_references,
        text: payload.text,
        min_confidence: options.min_confidence,
        alternatives: payload.alternatives,
        encoding_decision: payload.decision,
        segmentation_decisions: applied.segmentation_decisions,
        filler_decisions: applied.filler_decisions,
        edit_decisions: applied.edit_decisions,
        role_decisions: applied.role_decisions,
        answer_style_decision,
        research_decision,
        context_decisions,
        stats,
    })
}

/// Adds everything the standardizer contributes beyond the user's words, as notes.
fn annotate(
    value: &mut Json,
    draft: &Draft<'_>,
    response: Option<&SystemOneResponse>,
    options: &StandardizeOptions,
) -> (
    Option<GuidanceDecision>,
    Option<GuidanceDecision>,
    Vec<ContextDecision>,
) {
    let (answer_style, research) = guidance::apply(
        value,
        response,
        options.enhancements.response_guidance && draft.prose,
        options.min_confidence,
    );
    let background = questions::recent_background(&options.background);
    let context = if draft.prose {
        context::apply(value, response, background, options.min_confidence)
    } else {
        Vec::new()
    };
    (answer_style, research, context)
}

fn stats(
    draft: &Draft<'_>,
    applied: &Decided,
    payload: &Payload,
    (response, requests): (Option<&SystemOneResponse>, usize),
    options: &StandardizeOptions,
    started: Instant,
) -> StandardizeStats {
    let jev_called = response.is_some();
    let background_items = questions::recent_background(&options.background).len();
    StandardizeStats {
        input_chars: draft.input_chars,
        output_chars: payload.text.len(),
        input_tokens_estimate: estimate_tokens(&draft.input_text),
        output_tokens_estimate: estimate_tokens(&payload.text),
        background_items: if jev_called { background_items } else { 0 },
        segments: applied.segments,
        segmentation_candidates: draft.boundaries.len(),
        filler_candidates: draft.filler.len(),
        filler_removed: applied.filler_removed,
        edits_applied: applied.edits_applied,
        jev_called,
        jev_requests: requests,
        // Identical requests: each backup is billed for the same input tokens.
        jev_input_tokens: response
            .and_then(|response| response.usage.input_tokens)
            .unwrap_or(0)
            * u64::try_from(requests).unwrap_or(1),
        elapsed_ms: started.elapsed().as_millis(),
    }
}

fn validate_options(options: &StandardizeOptions) -> Result<()> {
    if !(0.5..=1.0).contains(&options.min_confidence) {
        return Err(StandardizeError::InvalidOption(
            "min_confidence must be between 0.5 and 1".to_owned(),
        ));
    }
    let limits = [
        options.max_segments,
        options.max_filler_candidates,
        options.max_input_chars,
    ];
    if limits.contains(&0) || options.request_timeout_ms == 0 {
        return Err(StandardizeError::InvalidOption(
            "limits and request timeout must be positive".to_owned(),
        ));
    }
    Ok(())
}
