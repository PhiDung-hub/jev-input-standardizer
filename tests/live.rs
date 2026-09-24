use jev_input_standardizer::{StandardizeOptions, standardize};
use typesafe_ai::{Client, Json};

#[tokio::test]
#[ignore = "requires TYPESAFE_API_KEY and makes a live API request"]
async fn live_standardization_uses_one_fanout_request() {
    let _ = dotenvy::dotenv();
    let context = Json::from(
        "Please implement the parser with the existing typed models.\n\nDo not change the public API.\n\nReturn the exact downstream preview.",
    );
    let options = StandardizeOptions {
        jev_min_chars: 0,
        ..StandardizeOptions::default()
    };
    let result = standardize(&Client::from_env().unwrap(), &context, &options)
        .await
        .unwrap();

    assert!(result.stats.jev_called);
    assert!(result.stats.jev_input_tokens > 0);
    eprintln!(
        "live standardization: {} segments, {} filler candidates, {} ms, {} Jev input tokens, {} -> {} estimated downstream tokens",
        result.stats.segments,
        result.stats.filler_candidates,
        result.stats.elapsed_ms,
        result.stats.jev_input_tokens,
        result.stats.input_tokens_estimate,
        result.stats.output_tokens_estimate,
    );
}
