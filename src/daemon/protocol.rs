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
    #[serde(default)]
    pub prefixes: Vec<String>,
    #[serde(default)]
    pub suffixes: Vec<String>,
    pub min_len: Option<usize>,
    pub max_len: Option<usize>,
}

impl QueryParams {
    /// Build all `DictionaryQuery` objects from the Cartesian product of
    /// prefixes and suffixes.  Falls back to the singular `prefix`/`suffix`
    /// fields when the plural vectors are empty.
    pub fn into_queries(self) -> Vec<crate::dictionary::DictionaryQuery> {
        let prefixes = if self.prefixes.is_empty() {
            vec![self.prefix]
        } else {
            self.prefixes.into_iter().map(Some).collect()
        };
        let suffixes = if self.suffixes.is_empty() {
            vec![self.suffix]
        } else {
            self.suffixes.into_iter().map(Some).collect()
        };

        prefixes
            .into_iter()
            .flat_map(|p| {
                suffixes
                    .iter()
                    .map(move |s| crate::dictionary::DictionaryQuery {
                        prefix: p.clone(),
                        suffix: s.clone(),
                        min_length: self.min_len,
                        max_length: self.max_len,
                    })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompletionOptions {
    pub input_prep: crate::text::TextPrep,
    pub context_prep: crate::text::TextPrep,
    pub case_preference: crate::text::CasePreference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteParams {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default = "default_max")]
    pub max: usize,
    #[serde(default)]
    pub options: CompletionOptions,
}

#[derive(Debug, Deserialize)]
pub struct LimitedQueryParams {
    #[serde(flatten)]
    pub query: QueryParams,
    pub max: usize,
}

#[derive(Debug, Deserialize)]
pub struct PredictParams {
    #[serde(default)]
    pub options: CompletionOptions,
    pub context: Vec<String>,
    #[serde(default = "default_max")]
    pub max: usize,
}

#[derive(Debug, Deserialize)]
pub struct FrequencyParams {
    pub word: String,
}

#[derive(Debug, Deserialize)]
pub struct WordAddParams {
    pub word: String,
    #[serde(default = "default_frequency")]
    pub frequency: f64,
    #[serde(default = "default_true")]
    pub allow_existing: bool,
}

#[derive(Debug, Deserialize)]
pub struct NgramBumpParams {
    pub ngram: Vec<String>,
    #[serde(default = "default_delta")]
    pub delta: f64,
    #[serde(default = "default_true")]
    pub save_unknown: bool,
}

fn default_max() -> usize {
    10
}

fn default_frequency() -> f64 {
    1.0
}

fn default_delta() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}
