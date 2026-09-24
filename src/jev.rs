use std::time::Duration;

use typesafe_ai::{Client, Questions, RequestOptions, RetryPolicy, SystemOneResponse};

use crate::draft::Draft;
use crate::encoding;
use crate::model::{Result, StandardizeOptions};
use crate::questions::{self, JudgmentState};

/// Jev's answers and how many identical requests it took to get them.
pub(crate) struct Judgment {
    pub response: SystemOneResponse,
    pub requests: usize,
}

/// Asks every judgment in one fan-out request. `None` means Jev was not needed.
pub(crate) async fn judge(
    client: &Client,
    draft: &Draft<'_>,
    options: &StandardizeOptions,
) -> Result<Option<Judgment>> {
    let data = encoding::data_candidates(draft, options)?;
    let background = questions::recent_background(&options.background);
    let question_set = questions::questions(draft, data.as_ref(), background, options);
    let short_question_clause = draft
        .boundaries
        .iter()
        .any(|boundary| boundary.cue == "question_clause");
    // A short draft goes as typed unless it may need structure: sentences already
    // guessed as different kinds (a question, then an instruction) are worth ~0.5 s.
    let looks_mixed = draft.prose && !encoding::one_kind(&draft.provisional);
    let below_minimum =
        draft.input_chars < options.jev_min_chars && !short_question_clause && !looks_mixed;
    if below_minimum || question_set.is_empty() {
        return Ok(None);
    }
    let state = questions::state(draft, data.as_ref(), background, options);
    hedged(client, &state, &question_set, options)
        .await
        .map(Some)
}

/// Jev's latency is service noise (median ~0.55 s, 90th percentile ~1.5 s), so a
/// request still pending at `hedge_after_ms` gets one identical backup and the first
/// answer wins; the backup's deadline keeps the overall timeout unchanged.
async fn hedged(
    client: &Client,
    state: &JudgmentState<'_>,
    questions: &Questions,
    options: &StandardizeOptions,
) -> Result<Judgment> {
    let timeout = Duration::from_millis(options.request_timeout_ms);
    let hedge_after = Duration::from_millis(options.hedge_after_ms);
    let mut request_options = RequestOptions::default()
        .retry(RetryPolicy::disabled())
        .timeout(timeout);
    if let Some(model) = options.model.as_deref() {
        request_options = request_options.model(model);
    }
    let first = client.system_one_with(state, questions, &request_options);
    tokio::pin!(first);
    if hedge_after.is_zero() || hedge_after >= timeout {
        let response = first.await?;
        return Ok(Judgment {
            response,
            requests: 1,
        });
    }
    tokio::select! {
        result = &mut first => return Ok(Judgment { response: result?, requests: 1 }),
        () = tokio::time::sleep(hedge_after) => {}
    }
    let backup_options = request_options
        .clone()
        .timeout(timeout.saturating_sub(hedge_after));
    let second = client.system_one_with(state, questions, &backup_options);
    tokio::pin!(second);
    let response = tokio::select! {
        result = &mut first => match result {
            Ok(response) => response,
            Err(_) => second.await?,
        },
        result = &mut second => match result {
            Ok(response) => response,
            Err(_) => first.await?,
        },
    };
    Ok(Judgment {
        response,
        requests: 2,
    })
}
