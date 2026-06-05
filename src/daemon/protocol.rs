use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single request received on stdin.
#[derive(Debug, Deserialize)]
pub struct DaemonRequest {
    pub id: Option<u64>,
    pub method: String,
    pub params: Value,
    /// Language override — takes precedence over the daemon's configured default.
    #[serde(default)]
    pub lang: Option<String>,
}

/// A single response written to stdout.
#[derive(Debug, Serialize)]
pub struct DaemonResponse {
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DaemonResponse {
    pub fn success(id: Option<u64>, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<u64>, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(message.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Param types for each method
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct IsCorrectParams {
    pub word: String,
}

#[derive(Debug, Deserialize)]
pub struct SuggestParams {
    pub word: String,
    #[serde(default = "default_max")]
    pub max: usize,
}

#[derive(Debug, Deserialize)]
pub struct QueryParams {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub min_len: Option<usize>,
    pub max_len: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct PredictParams {
    pub context: Vec<String>,
    #[serde(default = "default_max")]
    pub max: usize,
}

#[derive(Debug, Deserialize)]
pub struct FrequencyParams {
    pub word: String,
}

fn default_max() -> usize {
    10
}
