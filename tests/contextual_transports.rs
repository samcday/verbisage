#![cfg(all(feature = "dbus", feature = "sqlite"))]
use std::sync::Arc;
use verbisage::clients::{DbusClient, StdioClient};
use verbisage::completion::CompletionInput;
use verbisage::daemon::{DaemonHandler, dbus::VerbisageDbus};
use verbisage::dictionary::{DictionaryBackend, DictionaryQuery, PresageSqliteBackend};
use verbisage::prediction::{Predictor, smoothed::SmoothedPredictor};
use verbisage::text::TextPrep;

#[test]
fn real_stdio_and_dbus_clients_share_context_preparation_limits_and_chaining() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("en_US.db");
    let dict = Arc::new(PresageSqliteBackend::open(&path, true).unwrap());
    for (word, count) in [
        ("see", 100.0),
        ("you", 100.0),
        ("later", 5.0),
        ("large", 500.0),
        ("last", 400.0),
    ] {
        dict.add_word(word, count, true).unwrap();
    }
    let predictor = SmoothedPredictor::new(dict.clone());
    for (ngram, count) in [
        (vec!["see", "you"], 90.0),
        (vec!["you", "later"], 90.0),
        (vec!["see", "you", "later"], 85.0),
    ] {
        predictor
            .increase_ngram_frequency(&ngram, count, true)
            .unwrap();
    }
    let handler = DaemonHandler::new(
        Box::new(dict),
        None,
        Some(Box::new(predictor)),
        "en_US".into(),
    );
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let (stop, finished) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let _server = zbus::blocking::connection::Builder::unix_stream(a)
            .p2p()
            .server("0123456789abcdef0123456789abcdef")
            .unwrap()
            .serve_at("/org/verbisage/Dictionary", VerbisageDbus::new(handler))
            .unwrap()
            .build()
            .unwrap();
        finished.recv().unwrap();
    });
    let client = DbusClient::with_connection(
        zbus::blocking::connection::Builder::unix_stream(b)
            .p2p()
            .build()
            .unwrap(),
    );
    let mut stdio = StdioClient::spawn(&[
        "--mode",
        "stdio",
        "--backend",
        "sqlite",
        "--system-dict",
        path.to_str().unwrap(),
        "--user-dict",
        "",
        "--language",
        "en_US",
    ])
    .unwrap();
    for (word, context, expected) in [
        ("", vec!["See"], "you"),
        ("", vec!["see", "you"], "later"),
        ("L", vec!["see", "YOU"], "later"),
    ] {
        let input = CompletionInput {
            input: word,
            context: &context,
            input_prep: TextPrep::CLI_DEFAULT,
            context_prep: TextPrep::CLI_DEFAULT,
            ..Default::default()
        };
        let dbus = client.complete_with(&input, 6, "en_US").unwrap();
        let pipe = stdio.complete_with(&input, 6).unwrap();
        assert_eq!(dbus[0].0, expected);
        assert_eq!(
            dbus,
            pipe.into_iter()
                .map(|r| (r.word, r.confidence))
                .collect::<Vec<_>>()
        );
    }
    let dbus = client
        .predict_with(&["SEE", "YOU"], 6, "en_US", TextPrep::CLI_DEFAULT)
        .unwrap();
    let pipe = stdio
        .predict_with(&["SEE", "YOU"], 6, TextPrep::CLI_DEFAULT)
        .unwrap();
    assert_eq!(
        dbus,
        pipe.into_iter()
            .map(|r| (r.word, r.confidence))
            .collect::<Vec<_>>()
    );
    let input = CompletionInput {
        input: "l",
        ..Default::default()
    };
    assert!(
        client
            .complete_with(&input, 1001, "en_US")
            .unwrap_err()
            .to_string()
            .contains("cap")
    );
    assert!(
        stdio
            .complete_with(&input, 1001)
            .unwrap_err()
            .to_string()
            .contains("cap")
    );
    assert!(client.complete_with(&input, 0, "en_US").unwrap().is_empty());
    assert!(stdio.complete_with(&input, 0).unwrap().is_empty());
    let invalid = CompletionInput {
        context: &["two words"],
        ..Default::default()
    };
    assert!(client.complete_with(&invalid, 6, "en_US").is_err());
    assert!(stdio.complete_with(&invalid, 6).is_err());
    let query = DictionaryQuery {
        prefix: Some("l".into()),
        suffix: None,
        min_length: None,
        max_length: None,
    };
    assert_eq!(stdio.query_limited(&query, 2).unwrap().len(), 2);
    assert!(
        stdio
            .query_limited(&query, 200001)
            .unwrap_err()
            .to_string()
            .contains("cap")
    );
    drop(stdio);
    drop(client);
    stop.send(()).unwrap();
    server.join().unwrap();
}
