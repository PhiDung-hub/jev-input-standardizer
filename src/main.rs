use jev_input_standardizer::{StandardizeError, StandardizeOptions, standardize};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use typesafe_ai::{Client, Json};

#[derive(Deserialize)]
struct Input {
    context: Json,
    #[serde(default)]
    options: StandardizeOptions,
}

#[tokio::main]
async fn main() -> Result<(), StandardizeError> {
    let mut bytes = Vec::new();
    tokio::io::stdin().read_to_end(&mut bytes).await?;
    let input: Input = serde_json::from_slice(&bytes)?;
    let result = standardize(&Client::from_env()?, &input.context, &input.options).await?;
    let encoded = serde_json::to_vec(&result)?;
    let mut output = tokio::io::stdout();
    output.write_all(&encoded).await?;
    output.write_all(b"\n").await?;
    Ok(())
}
