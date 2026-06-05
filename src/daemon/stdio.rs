use std::io::{self, BufRead, BufWriter, Write};

use super::handlers::DaemonHandler;
use super::protocol::DaemonRequest;

/// Run the daemon event loop over stdin / stdout.
///
/// Reads one JSON object per line from stdin, dispatches via `handler`,
/// and writes one JSON object per line to stdout.  Malformed lines produce
/// an error response with `id: null`.
pub fn run(handler: DaemonHandler) {
    eprintln!("[daemon] stdio event loop started");
    let stdin = io::stdin().lock();
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    for line in stdin.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                // I/O error on stdin is fatal.
                eprintln!("[verbisaged] stdin error: {}", e);
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let req: DaemonRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let err_resp =
                    super::protocol::DaemonResponse::error(None, format!("parse error: {}", e));
                let serialized = serde_json::to_string(&err_resp).unwrap();
                let _ = writeln!(writer, "{}", serialized);
                let _ = writer.flush();
                continue;
            }
        };

        let resp = handler.handle(req);
        crate::veprintln!("[daemon] -> {}", serde_json::to_string(&resp).unwrap());
        let serialized = serde_json::to_string(&resp).unwrap();
        if let Err(e) = writeln!(writer, "{}", serialized) {
            eprintln!("[verbisaged] write error: {}", e);
            break;
        }
        if let Err(e) = writer.flush() {
            eprintln!("[verbisaged] flush error: {}", e);
            break;
        }
    }
}
