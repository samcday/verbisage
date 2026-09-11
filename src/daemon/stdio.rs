use std::io::{self, BufRead, BufWriter, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use super::handlers::DaemonHandler;
use super::protocol::{DaemonRequest, DaemonResponse};

struct BusyGuard(Arc<AtomicBool>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct CompletionWorker {
    sender: mpsc::SyncSender<(DaemonRequest, mpsc::Sender<DaemonResponse>, BusyGuard)>,
    busy: Arc<AtomicBool>,
    deadline: Duration,
}

impl CompletionWorker {
    fn new(handler: Arc<DaemonHandler>, deadline: Duration) -> Self {
        let (sender, receiver) =
            mpsc::sync_channel::<(DaemonRequest, mpsc::Sender<DaemonResponse>, BusyGuard)>(1);
        std::thread::spawn(move || {
            while let Ok((request, reply, permit)) = receiver.recv() {
                let response = handler.handle(request);
                drop(permit);
                let _ = reply.send(response);
            }
        });
        Self {
            sender,
            busy: Arc::new(AtomicBool::new(false)),
            deadline,
        }
    }

    fn request(&self, request: DaemonRequest) -> DaemonResponse {
        let id = request.id;
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return DaemonResponse::error(id, "completion worker is busy");
        }
        let permit = BusyGuard(self.busy.clone());
        let (reply, receiver) = mpsc::channel();
        if self.sender.try_send((request, reply, permit)).is_err() {
            return DaemonResponse::error(id, "completion worker unavailable");
        }
        match receiver.recv_timeout(self.deadline) {
            Ok(response) => response,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                DaemonResponse::error(id, "completion exceeded its response deadline")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                DaemonResponse::error(id, "completion worker failed")
            }
        }
    }
}

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
    let handler = Arc::new(handler);
    let completion = CompletionWorker::new(handler.clone(), handler.completion_deadline());

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

        // A timed-out query keeps its sole worker occupied until it actually
        // finishes. The input loop can report busy without spawning more work.
        // Mutations retain their synchronous, ordered acknowledgement semantics.
        let resp = if matches!(
            req.method.as_str(),
            "complete" | "complete_with" | "predict" | "predict_with"
        ) && req.params.get("max").and_then(serde_json::Value::as_u64) != Some(0)
        {
            completion.request(req)
        } else {
            handler.handle(req)
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    struct BlockedDictionary {
        release: Arc<AtomicBool>,
        entered: Arc<AtomicUsize>,
    }
    impl DictionaryBackend for BlockedDictionary {
        fn query_prefixes(&self, _: &[DictionaryQuery]) -> Vec<DictionaryResult> {
            self.entered.fetch_add(1, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            vec![DictionaryResult {
                word: "hello".into(),
                confidence: 1.0,
            }]
        }
        fn get_frequency(&self, _: &str) -> f64 {
            1.0
        }
        fn contains(&self, _: &str) -> bool {
            false
        }
    }
    struct ReleaseOnDrop(Arc<AtomicBool>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    fn request(id: u64) -> DaemonRequest {
        DaemonRequest {
            id: Some(id),
            method: "predict_with".into(),
            params: serde_json::json!({"context": [], "max": 6}),
            lang: None,
        }
    }

    #[test]
    fn deadline_keeps_worker_bounded_and_discards_late_reply() {
        let release = Arc::new(AtomicBool::new(false));
        let _release_on_failure = ReleaseOnDrop(release.clone());
        let entered = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(DaemonHandler::new(
            Box::new(BlockedDictionary {
                release: release.clone(),
                entered: entered.clone(),
            }),
            None,
            None,
            "en_US".into(),
        ));
        let worker = CompletionWorker::new(handler, Duration::from_millis(50));
        let start = Instant::now();
        let first = worker.request(request(1));
        assert!(first.error.unwrap().contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(first.id, Some(1));
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        for _ in 0..10 {
            assert!(worker.request(request(2)).error.unwrap().contains("busy"));
        }
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        release.store(true, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(1);
        while worker.busy.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!worker.busy.load(Ordering::Acquire));
        let next = worker.request(request(3));
        assert_eq!(next.id, Some(3));
        assert!(next.error.is_none(), "{next:?}");
        assert_eq!(next.result.unwrap()[0]["word"], "hello");
    }
}
