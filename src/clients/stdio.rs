use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::dictionary::{DictionaryQuery, DictionaryResult};
use crate::prediction::Prediction;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ClientError {
    Io(std::io::Error),
    Spawn(String),
    Protocol(String),
    Json(serde_json::Error),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "{}", e),
            ClientError::Spawn(s) => write!(f, "spawn error: {}", s),
            ClientError::Protocol(s) => write!(f, "protocol error: {}", s),
            ClientError::Json(e) => write!(f, "json error: {}", e),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Io(e) => Some(e),
            ClientError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Io(e)
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> Self {
        ClientError::Json(e)
    }
}

// ---------------------------------------------------------------------------
// StdioClient
// ---------------------------------------------------------------------------

/// Client that communicates with a `verbisaged` daemon over stdin/stdout.
///
/// Spawns the daemon as a child process and exchanges line-delimited JSON.
/// Each method sends a request with a monotonically increasing id and
/// blocks until the response arrives.
///
/// # Example
///
/// ```no_run
/// use verbisage::clients::StdioClient;
///
/// let mut client = StdioClient::spawn(&[
///     "--backend", "file",
///     "--path", "/usr/share/dict/words",
/// ]).unwrap();
///
/// assert!(client.is_correct("hello").unwrap());
/// ```
pub struct StdioClient {
    process: Child,
    writer: BufWriter<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: AtomicU64,
}

impl StdioClient {
    /// Spawn `verbisaged` with `args` and connect to its stdin/stdout.
    ///
    /// The binary is looked up relative to the current executable first,
    /// then via `PATH`.
    pub fn spawn(args: &[&str]) -> Result<Self, ClientError> {
        let exe = find_binary();

        let mut cmd = Command::new(&exe);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ClientError::Spawn("failed to capture stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ClientError::Spawn("failed to capture stdout".into()))?;

        Ok(Self {
            process: child,
            writer: BufWriter::new(stdin),
            reader: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
        })
    }

    /// Check whether `word` is in the dictionary.
    pub fn is_correct(&mut self, word: &str) -> Result<bool, ClientError> {
        let v = self.send_request("is_correct", json!({"word": word}))?;
        serde_json::from_value(v).map_err(ClientError::Json)
    }

    /// Request spelling suggestions for `word`.
    pub fn suggest(&mut self, word: &str, max: usize) -> Result<Vec<String>, ClientError> {
        let v = self.send_request("suggest", json!({"word": word, "max": max}))?;
        serde_json::from_value(v).map_err(ClientError::Json)
    }

    /// Query the dictionary with prefix / suffix / length constraints.
    pub fn query(&mut self, query: &DictionaryQuery) -> Result<Vec<DictionaryResult>, ClientError> {
        let params = json!({
            "prefix": query.prefix,
            "suffix": query.suffix,
            "min_len": query.min_length,
            "max_len": query.max_length,
        });
        let v = self.send_request("query", params)?;
        let items: Vec<Value> = serde_json::from_value(v)?;
        items
            .into_iter()
            .map(|item| {
                Ok(DictionaryResult {
                    word: item["word"].as_str().unwrap_or_default().to_string(),
                    confidence: item["confidence"].as_f64().unwrap_or(-1.0),
                })
            })
            .collect()
    }

    /// Predict the next word(s) given `context`.
    pub fn predict(
        &mut self,
        context: &[&str],
        max: usize,
    ) -> Result<Vec<Prediction>, ClientError> {
        let ctx: Vec<String> = context.iter().map(|s| s.to_string()).collect();
        let params = json!({"context": ctx, "max": max});
        let v = self.send_request("predict", params)?;
        // parse {word, confidence} items
        let items: Vec<Value> = serde_json::from_value(v)?;
        items
            .into_iter()
            .map(|item| {
                Ok(Prediction {
                    word: item["word"].as_str().unwrap_or_default().to_string(),
                    confidence: item["confidence"].as_f64().unwrap_or(0.0),
                })
            })
            .collect()
    }

    /// Look up the frequency of `word`.
    pub fn frequency(&mut self, word: &str) -> Result<f64, ClientError> {
        let v = self.send_request("frequency", json!({"word": word}))?;
        serde_json::from_value(v).map_err(ClientError::Json)
    }

    // ── internal ────────────────────────────────────────────────────────

    fn send_request(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let line = serde_json::to_string(&request)?;
        writeln!(self.writer, "{}", line)?;
        self.writer.flush()?;

        let mut resp_line = String::new();
        self.reader.read_line(&mut resp_line)?;

        if resp_line.is_empty() {
            return Err(ClientError::Protocol(
                "daemon closed connection without response".into(),
            ));
        }

        let resp: Value = serde_json::from_str(&resp_line)?;

        if let Some(err) = resp.get("error").and_then(|e| e.as_str()) {
            return Err(ClientError::Protocol(err.to_string()));
        }

        let resp_id = resp["id"].as_u64().unwrap_or(0);
        if resp_id != id {
            return Err(ClientError::Protocol(format!(
                "id mismatch: sent {} got {}",
                id, resp_id
            )));
        }

        Ok(resp["result"].clone())
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

// ── binary locator ────────────────────────────────────────────────────────

fn find_binary() -> String {
    // Guess: test binary lives at target/debug/deps/verbisage-<hash>,
    //        production binary at target/debug/verbisaged.
    //        Walk up at most 2 parent directories.
    let exe = std::env::current_exe().ok();
    if let Some(path) = exe {
        if let Some(dir) = path.parent() {
            let candidate = dir.join("verbisaged");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
            // Try target/debug/verbisaged from target/debug/deps/verbisage-*
            if let Some(parent) = dir.parent() {
                let candidate = parent.join("verbisaged");
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
    }
    "verbisaged".to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a temporary word-list file and test all client methods.
    #[test]
    fn stdio_client_basic() {
        let dir = std::env::temp_dir();
        let dict_path = dir.join("verbisage_test_dict.txt");
        {
            let mut f = std::fs::File::create(&dict_path).unwrap();
            writeln!(f, "hello").unwrap();
            writeln!(f, "world").unwrap();
            writeln!(f, "help").unwrap();
            writeln!(f, "helium").unwrap();
        }

        let mut client =
            StdioClient::spawn(&["--backend", "file", "--path", &dict_path.to_string_lossy()])
                .unwrap();

        // is_correct
        assert!(client.is_correct("hello").unwrap());
        assert!(!client.is_correct("xyzzy").unwrap());

        // suggest
        let suggestions = client.suggest("helo", 5).unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"hello".to_string()));

        // frequency
        let freq = client.frequency("hello").unwrap();
        assert!(freq > 0.0);
        let freq_missing = client.frequency("nonexistent").unwrap();
        assert_eq!(freq_missing, 0.0);

        // query — prefix only
        let results = client
            .query(&DictionaryQuery {
                prefix: Some("hel".to_string()),
                suffix: None,
                min_length: None,
                max_length: None,
            })
            .unwrap();
        assert!(results.len() >= 3);
        assert!(results.iter().any(|r| r.word == "hello"));

        // query — prefix + suffix
        let results = client
            .query(&DictionaryQuery {
                prefix: Some("hel".to_string()),
                suffix: Some("o".to_string()),
                min_length: Some(4),
                max_length: Some(6),
            })
            .unwrap();
        assert!(results.iter().any(|r| r.word == "hello"));

        // query — length only
        let results = client
            .query(&DictionaryQuery {
                prefix: None,
                suffix: None,
                min_length: Some(4),
                max_length: Some(4),
            })
            .unwrap();
        assert!(results.iter().any(|r| r.word == "help"));

        let _ = std::fs::remove_file(&dict_path);
    }

    /// Error responses from the daemon are propagated.
    #[test]
    fn stdio_client_unknown_method() {
        let dir = std::env::temp_dir();
        let dict_path = dir.join("verbisage_test_unknown.txt");
        {
            let mut f = std::fs::File::create(&dict_path).unwrap();
            writeln!(f, "test").unwrap();
        }

        // Manually craft an invalid request to test error handling.
        let exe = find_binary();
        let mut child = Command::new(&exe)
            .args(&["--backend", "file", "--path", &dict_path.to_string_lossy()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let stdin = child.stdin.take().unwrap();
        let mut writer = BufWriter::new(stdin);
        let mut reader = BufReader::new(child.stdout.take().unwrap());

        // Send a request with an unknown method
        let req = json!({"id": 1, "method": "nonexistent", "params": {}});
        writeln!(writer, "{}", serde_json::to_string(&req).unwrap()).unwrap();
        writer.flush().unwrap();

        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert!(resp["error"].is_string());

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&dict_path);
    }
}
