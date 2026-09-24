//! Loopback bridge for explicit prompt standardization.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use jev_input_standardizer::{
    StandardizeError, StandardizeOptions, StandardizeResult, standardize,
};
use serde::{Deserialize, Serialize};
use typesafe_ai::{Client, Json as TypeSafeJson};

const DEFAULT_LISTEN: &str = "127.0.0.1:8788";
const MAX_REQUEST_BYTES: usize = 128 * 1024;

#[derive(Clone)]
struct AppState {
    client: Client,
    usage_paused: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    attempts: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    jev_requests: AtomicU64,
    jev_input_tokens: AtomicU64,
    input_tokens_estimate: AtomicU64,
    output_tokens_estimate: AtomicU64,
    filler_removed: AtomicU64,
    salient_weight: AtomicU64,
    retained_weight: AtomicU64,
    elapsed_ms: AtomicU64,
    last_elapsed_ms: AtomicU64,
}

impl Metrics {
    fn success(&self, result: &StandardizeResult, salient: u64, retained: u64) {
        let stats = &result.stats;
        self.succeeded.fetch_add(1, Ordering::Relaxed);
        self.jev_requests
            .fetch_add(u64::from(stats.jev_called), Ordering::Relaxed);
        self.jev_input_tokens
            .fetch_add(stats.jev_input_tokens, Ordering::Relaxed);
        self.input_tokens_estimate
            .fetch_add(as_u64(stats.input_tokens_estimate), Ordering::Relaxed);
        self.output_tokens_estimate
            .fetch_add(as_u64(stats.output_tokens_estimate), Ordering::Relaxed);
        self.filler_removed
            .fetch_add(as_u64(stats.filler_removed), Ordering::Relaxed);
        self.salient_weight.fetch_add(salient, Ordering::Relaxed);
        self.retained_weight.fetch_add(retained, Ordering::Relaxed);
        let elapsed = as_u64(stats.elapsed_ms);
        self.elapsed_ms.fetch_add(elapsed, Ordering::Relaxed);
        self.last_elapsed_ms.store(elapsed, Ordering::Relaxed);
    }

    fn snapshot(&self) -> Usage {
        Usage {
            attempts: self.attempts.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            jev_requests: self.jev_requests.load(Ordering::Relaxed),
            jev_input_tokens: self.jev_input_tokens.load(Ordering::Relaxed),
            input_tokens_estimate: self.input_tokens_estimate.load(Ordering::Relaxed),
            output_tokens_estimate: self.output_tokens_estimate.load(Ordering::Relaxed),
            filler_removed: self.filler_removed.load(Ordering::Relaxed),
            salient_weight: self.salient_weight.load(Ordering::Relaxed),
            retained_weight: self.retained_weight.load(Ordering::Relaxed),
            elapsed_ms: self.elapsed_ms.load(Ordering::Relaxed),
            last_elapsed_ms: self.last_elapsed_ms.load(Ordering::Relaxed),
        }
    }
}

fn as_u64(value: impl TryInto<u64>) -> u64 {
    value.try_into().unwrap_or(u64::MAX)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StandardizeRequest {
    context: TypeSafeJson,
    #[serde(default)]
    options: StandardizeOptions,
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    usage: Usage,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    attempts: u64,
    succeeded: u64,
    failed: u64,
    jev_requests: u64,
    jev_input_tokens: u64,
    input_tokens_estimate: u64,
    output_tokens_estimate: u64,
    filler_removed: u64,
    salient_weight: u64,
    retained_weight: u64,
    elapsed_ms: u64,
    last_elapsed_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BridgeError {
    code: &'static str,
    message: String,
}

#[derive(Debug, thiserror::Error)]
enum ServerError {
    #[error(transparent)]
    Address(#[from] std::net::AddrParseError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    TypeSafe(#[from] typesafe_ai::Error),
    #[error("HTTP client: {0}")]
    Http(String),
}

#[tokio::main]
async fn main() -> Result<(), ServerError> {
    let address = std::env::var("JEV_STANDARDIZER_LISTEN")
        .unwrap_or_else(|_| DEFAULT_LISTEN.to_owned())
        .parse::<SocketAddr>()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    let app = app(Client::builder().http_client(warm_http()?).build()?);
    eprintln!("jev-input-standardizer listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Alt+G presses are minutes apart, beyond reqwest's 90 s pool idle timeout, so each
/// would pay a fresh TCP and TLS handshake (~0.5 s to the Oregon API). Keep the pooled connection.
fn warm_http() -> Result<reqwest::Client, ServerError> {
    reqwest::Client::builder()
        .pool_idle_timeout(None)
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| ServerError::Http(error.to_string()))
}

fn app(client: Client) -> Router {
    Router::new()
        .route("/health", get(health_route))
        .route("/standardize", post(standardize_route))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(AppState {
            client,
            usage_paused: Arc::new(AtomicBool::new(false)),
            metrics: Arc::new(Metrics::default()),
        })
}

async fn health_route(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        status: if state.usage_paused.load(Ordering::Acquire) {
            "usage_exhausted"
        } else {
            "ready"
        },
        usage: state.metrics.snapshot(),
    })
}

async fn standardize_route(
    State(state): State<AppState>,
    Json(request): Json<StandardizeRequest>,
) -> Result<Json<StandardizeResult>, (StatusCode, Json<BridgeError>)> {
    let request_id = state.metrics.attempts.fetch_add(1, Ordering::Relaxed) + 1;
    if let Err(error) = ensure_ready(&state) {
        state.metrics.failed.fetch_add(1, Ordering::Relaxed);
        eprintln!(
            "standardize request={request_id} status=failed http={} code={}",
            error.0.as_u16(),
            error.1.0.code
        );
        return Err(error);
    }
    match run(&state, &request.context, &request.options).await {
        Ok(result) => {
            eprintln!(
                "standardize request={request_id} status=ok target_model={} background_items={} encoding={} source={} probabilities={:?} segments={} boundary_candidates={} filler_candidates={} filler_removed={} input_tokens={} output_tokens={} jev_requests={} jev_input_tokens={} elapsed_ms={}",
                result.target_model.as_deref().unwrap_or("unknown"),
                result.stats.background_items,
                result.encoding.as_str(),
                result.encoding_decision.source.as_str(),
                result.encoding_decision.probabilities,
                result.stats.segments,
                result.stats.segmentation_candidates,
                result.stats.filler_candidates,
                result.stats.filler_removed,
                result.stats.input_tokens_estimate,
                result.stats.output_tokens_estimate,
                result.stats.jev_requests,
                result.stats.jev_input_tokens,
                result.stats.elapsed_ms
            );
            // Only aggregate weights leave this request. The original text,
            // output text, and individual terms are never persisted here.
            let source = match &request.context {
                TypeSafeJson::String(text) => text.clone(),
                value => serde_json::to_string(value).unwrap_or_default(),
            };
            let (salient, retained) = weighted_retention(&source, &result.text);
            state.metrics.success(&result, salient, retained);
            Ok(Json(result))
        }
        Err(error) => {
            state.metrics.failed.fetch_add(1, Ordering::Relaxed);
            // The message says why (timeout, connection, API status); it never holds the key.
            eprintln!(
                "standardize request={request_id} status=failed http={} code={} message={:?}",
                error.0.as_u16(),
                error.1.0.code,
                error.1.0.message
            );
            Err(error)
        }
    }
}

/// A local key-term preservation proxy, not a semantic quality judgment.
/// Deduplicating terms keeps repeated filler from overwhelming paths, numbers,
/// identifiers and negations. No token or source text is stored in metrics.
fn weighted_retention(source: &str, output: &str) -> (u64, u64) {
    fn terms(text: &str) -> HashSet<String> {
        text.split(|ch: char| !ch.is_alphanumeric() && !matches!(ch, '_' | '-' | '.'))
            .filter(|term| term.chars().count() >= 2)
            .map(str::to_lowercase)
            .filter(|term| {
                !matches!(
                    term.as_str(),
                    "the"
                        | "and"
                        | "for"
                        | "with"
                        | "from"
                        | "this"
                        | "that"
                        | "are"
                        | "was"
                        | "were"
                        | "you"
                        | "your"
                        | "but"
                        | "have"
                        | "has"
                        | "into"
                )
            })
            .collect()
    }
    fn weight(term: &str) -> u64 {
        if term.chars().any(|ch| ch.is_ascii_digit()) {
            4
        } else if matches!(term, "not" | "never" | "only" | "must" | "without")
            || term.contains(['/', '_', '-', '.'])
        {
            3
        } else if term.chars().count() >= 8 {
            2
        } else {
            1
        }
    }
    let source = terms(source);
    let output = terms(output);
    let salient = source.iter().map(|term| weight(term)).sum();
    let retained = source.intersection(&output).map(|term| weight(term)).sum();
    (salient, retained)
}

async fn run(
    state: &AppState,
    context: &TypeSafeJson,
    options: &StandardizeOptions,
) -> Result<StandardizeResult, (StatusCode, Json<BridgeError>)> {
    match standardize(&state.client, context, options).await {
        Ok(result) => Ok(result),
        Err(error) if is_usage_exhausted(&error) => {
            state.usage_paused.store(true, Ordering::Release);
            Err(bridge_error(
                StatusCode::PAYMENT_REQUIRED,
                "usage_exhausted",
                error.to_string(),
            ))
        }
        Err(
            error @ (StandardizeError::InputTooLarge { .. } | StandardizeError::InvalidOption(_)),
        ) => Err(bridge_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            error.to_string(),
        )),
        Err(error) => Err(bridge_error(
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            error.to_string(),
        )),
    }
}

fn ensure_ready(state: &AppState) -> Result<(), (StatusCode, Json<BridgeError>)> {
    if state.usage_paused.load(Ordering::Acquire) {
        return Err(bridge_error(
            StatusCode::PAYMENT_REQUIRED,
            "usage_exhausted",
            "TypeSafe usage circuit is paused".to_owned(),
        ));
    }
    Ok(())
}

fn is_usage_exhausted(error: &StandardizeError) -> bool {
    let StandardizeError::TypeSafe(error) = error else {
        return false;
    };
    let Some(api) = error.api_error() else {
        return false;
    };
    api.status == StatusCode::PAYMENT_REQUIRED.as_u16()
        || (api.status == StatusCode::FORBIDDEN.as_u16()
            && [
                "quota",
                "credit",
                "billing",
                "usage",
                "payment",
                "insufficient",
            ]
            .iter()
            .any(|word| api.message.to_ascii_lowercase().contains(word)))
}

fn bridge_error(
    status: StatusCode,
    code: &'static str,
    message: String,
) -> (StatusCode, Json<BridgeError>) {
    (status, Json(BridgeError { code, message }))
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use httpmock::{Method::POST, MockServer};
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn local_retention_weights_critical_terms_without_storing_text() {
        let source = "Never delete config_v2 at /tmp/path; keep 42 records";
        let unchanged = weighted_retention(source, source);
        let changed = weighted_retention(source, "Keep records at /tmp/path");
        assert!(unchanged.0 > 0);
        assert_eq!(unchanged.0, unchanged.1);
        assert_eq!(changed.0, unchanged.0);
        assert!(changed.1 < changed.0);
    }

    #[tokio::test]
    async fn standardize_route_returns_replacement_payload() {
        let upstream = MockServer::start_async().await;
        let mock = upstream
            .mock_async(|when, then| {
                when.method(POST).path("/v1/systemone");
                then.status(200).json_body(serde_json::json!({
                    "model": "jev-test",
                    "usage": {"input_tokens": 120, "output_tokens": 8},
                    "answers": {
                        "encoding": {
                            "type": "choice", "choice": "toon", "confidence": 0.9,
                            "probabilities": {"json": 0.1, "toon": 0.9}
                        },
                        "role_s1": {
                            "type": "choice", "choice": "task", "confidence": 0.9,
                            "probabilities": {"task": 0.9, "context": 0.1}
                        },
                        "drop_f1": {"type": "noul", "noul": 0.95}
                    }
                }));
            })
            .await;
        let client = Client::builder()
            .api_key("test")
            .base_url(upstream.base_url())
            .build()
            .unwrap();
        let prompt = format!(
            "Please implement the parser carefully. {}",
            "context ".repeat(30)
        );
        let request = Request::post("/standardize")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "context": prompt,
                    "options": {"jevMinChars": 0}
                })
                .to_string(),
            ))
            .unwrap();
        let app = app(client);
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["encoding"], "plain");
        assert_eq!(body["encodingDecision"]["source"], "heuristic");
        assert!(
            body["text"]
                .as_str()
                .unwrap()
                .starts_with("implement the parser")
        );
        assert_eq!(body["alternatives"][1]["encoding"], "xml");

        let health = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = to_bytes(health.into_body(), 1_000_000).await.unwrap();
        let health: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(health["status"], "ready");
        assert_eq!(health["usage"]["attempts"], 1);
        assert_eq!(health["usage"]["succeeded"], 1);
        assert_eq!(health["usage"]["jevRequests"], 1);
        assert_eq!(health["usage"]["jevInputTokens"], 120);
        assert!(health["usage"]["salientWeight"].as_u64().unwrap() > 0);
        assert!(health["usage"]["retainedWeight"].as_u64().unwrap() > 0);
        mock.assert_calls_async(1).await;
    }
}
