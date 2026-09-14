#![cfg(feature = "swipe")]
//! Registered shared layouts drive gesture recognition through the real
//! daemon handler, the real D-Bus interface over a peer-to-peer connection,
//! and the real Patricia backend. Dictionaries are temporary fixtures with
//! distinct words; nothing on the system is read or written.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use verbisage::daemon::dbus::VerbisageDbus;
use verbisage::daemon::{DaemonConfig, DaemonHandler};
use verbisage::dictionary::patricia::PatriciaDictionaryBackend;
use verbisage::dictionary::{DictionaryBackend, DictionaryQuery, DictionaryResult};
use verbisage::layout::{KeyBox, LayoutUpload};
use verbisage::swipe::{SwipeCandidate, SwipeVocabulary};
use zbus::zvariant::{DynamicType, Type};

const PATH: &str = "/org/verbisage/Dictionary";
const INTERFACE: &str = "org.verbisage.Dictionary1";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

type Trace = Vec<(f64, f64, u32)>;
type Row<'a> = &'a [(&'a str, &'a [&'a str])];

/// The frozen real Stevia US normal-layer export, copied unchanged. See
/// `tests/fixtures/README.md` for its identity and generation provenance.
const EXPORTED_US_NORMAL: &str = include_str!("fixtures/layout-us-normal.json");

/// The frozen export exactly as the client generated it: real widget
/// rectangles and the exporter's long-press labels (the period key carries an
/// apostrophe).
fn exported_us_normal() -> LayoutUpload {
    serde_json::from_str(EXPORTED_US_NORMAL).expect("frozen Stevia US normal-layer export")
}

/// The same export with the apostrophe removed from the period key's
/// long-press menu. Every other label and rectangle is untouched, so the only
/// difference is that an apostrophe now has no key at all.
fn exported_us_normal_without_apostrophe() -> LayoutUpload {
    let mut upload = exported_us_normal();
    for key in upload.keys.as_mut().expect("keys") {
        key.alt_labels.retain(|label| label != "'");
    }
    upload
}

fn dictionary(dir: &Path, tag: &str, words: &[(&str, u8)]) -> PathBuf {
    let path = dir.join(format!("{tag}.dict"));
    let mut dict = patricia_dict::Dictionary::create_empty_v403(&path, tag).unwrap();
    for (word, probability) in words {
        dict.append(word, *probability).unwrap();
    }
    path
}

/// Key rectangles in widget coordinates: 36x46 keys on a 40-unit pitch, each
/// row indented by 20, optionally scaled and translated.
fn keyboard(rows: &[Row<'_>], scale: f64, offset: (f64, f64)) -> LayoutUpload {
    let mut keys = Vec::new();
    for (row, labels) in rows.iter().enumerate() {
        for (col, (label, alternates)) in labels.iter().enumerate() {
            keys.push(KeyBox {
                label: (*label).to_string(),
                alt_labels: alternates.iter().map(|alt| (*alt).to_string()).collect(),
                left: ((col as f64 * 40.0 + row as f64 * 20.0) * scale + offset.0) as f32,
                top: (row as f64 * 50.0 * scale + offset.1) as f32,
                width: (36.0 * scale) as f32,
                height: (46.0 * scale) as f32,
            });
        }
    }
    LayoutUpload {
        keys: Some(keys),
        rows: None,
        ignored_labels: Vec::new(),
    }
}

/// A QWERTY layout whose listed keys carry alternate labels.
fn qwerty(alternates: &[(&str, &[&str])], scale: f64, offset: (f64, f64)) -> LayoutUpload {
    let rows: Vec<Vec<(String, Vec<String>)>> = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]
        .iter()
        .map(|letters| {
            letters
                .chars()
                .map(|letter| {
                    let label = letter.to_string();
                    let alts = alternates
                        .iter()
                        .find(|(key, _)| *key == label)
                        .map(|(_, alts)| alts.iter().map(|alt| (*alt).to_string()).collect())
                        .unwrap_or_default();
                    (label, alts)
                })
                .collect()
        })
        .collect();
    let borrowed: Vec<Vec<(&str, Vec<&str>)>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|(label, alts)| (label.as_str(), alts.iter().map(String::as_str).collect()))
                .collect()
        })
        .collect();
    let slices: Vec<Vec<(&str, &[&str])>> = borrowed
        .iter()
        .map(|row| {
            row.iter()
                .map(|(label, alts)| (*label, alts.as_slice()))
                .collect()
        })
        .collect();
    let refs: Vec<Row<'_>> = slices.iter().map(Vec::as_slice).collect();
    keyboard(&refs, scale, offset)
}

fn uppercase(upload: &LayoutUpload) -> LayoutUpload {
    let keys = upload
        .keys
        .as_ref()
        .unwrap()
        .iter()
        .map(|key| KeyBox {
            label: key.label.to_uppercase(),
            alt_labels: key
                .alt_labels
                .iter()
                .map(|alt| alt.to_uppercase())
                .collect(),
            ..key.clone()
        })
        .collect();
    LayoutUpload {
        keys: Some(keys),
        rows: None,
        ignored_labels: upload.ignored_labels.clone(),
    }
}

fn center(upload: &LayoutUpload, label: &str) -> (f64, f64) {
    let key = upload
        .keys
        .as_ref()
        .unwrap()
        .iter()
        .find(|key| key.label == label || key.alt_labels.iter().any(|alt| alt == label))
        .unwrap_or_else(|| panic!("no key labelled {label:?}"));
    (
        f64::from(key.left + key.width / 2.0),
        f64::from(key.top + key.height / 2.0),
    )
}

/// A key-center path through the labelled keys, six samples per segment.
fn trace(upload: &LayoutUpload, labels: &[&str]) -> Trace {
    let mut points: Trace = Vec::new();
    for label in labels {
        let target = center(upload, label);
        if let Some((x, y, _)) = points.last().copied() {
            if target == (x, y) {
                continue;
            }
            for step in 1..=6 {
                points.push((
                    x + (target.0 - x) * step as f64 / 6.0,
                    y + (target.1 - y) * step as f64 / 6.0,
                    points.len() as u32 * 10,
                ));
            }
        } else {
            points.push((target.0, target.1, 0));
        }
    }
    points
}

fn json(upload: &LayoutUpload) -> String {
    serde_json::to_string(upload).unwrap()
}

fn words(results: &[(String, f64)]) -> Vec<&str> {
    results.iter().map(|(word, _)| word.as_str()).collect()
}

/// A method error as the client sees it: the D-Bus error name and message.
#[derive(Debug)]
struct ServiceError {
    name: String,
    message: String,
}

impl ServiceError {
    fn invalid_args(&self) -> bool {
        self.name == INVALID_ARGS
    }
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

/// The real interface object served on a peer-to-peer connection, the way the
/// daemon serves it: registered from inside a Tokio runtime, driven by zbus's
/// own executor, recognition workers on the captured runtime handle.
struct Service {
    client: zbus::blocking::Connection,
    stop: Option<mpsc::Sender<()>>,
    server: Option<JoinHandle<()>>,
}

impl Service {
    fn start(handler: DaemonHandler) -> Self {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let (stop, finished) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let dbus = runtime.block_on(async { VerbisageDbus::new(handler) });
            let _server = zbus::blocking::connection::Builder::unix_stream(a)
                .p2p()
                .server("0123456789abcdef0123456789abcdef")
                .unwrap()
                .serve_at(PATH, dbus)
                .unwrap()
                .build()
                .unwrap();
            finished.recv().unwrap();
        });
        let client = zbus::blocking::connection::Builder::unix_stream(b)
            .p2p()
            .build()
            .unwrap();
        Self {
            client,
            stop: Some(stop),
            server: Some(server),
        }
    }

    fn patricia(words: &[(&str, u8)], lang: &str) -> (tempfile::TempDir, Self) {
        let temp = tempfile::tempdir().unwrap();
        let path = dictionary(temp.path(), lang, words);
        let backend = PatriciaDictionaryBackend::open(&path).unwrap();
        let handler = DaemonHandler::new(Box::new(backend), None, None, lang.into());
        (temp, Self::start(handler))
    }

    fn call<B, R>(&self, method: &str, body: &B) -> Result<R, ServiceError>
    where
        B: Serialize + DynamicType,
        R: DeserializeOwned + Type,
    {
        match self
            .client
            .call_method(None::<&str>, PATH, Some(INTERFACE), method, body)
        {
            Ok(message) => Ok(message.body().deserialize().unwrap()),
            Err(zbus::Error::MethodError(name, message, _)) => Err(ServiceError {
                name: name.to_string(),
                message: message.unwrap_or_default(),
            }),
            Err(other) => panic!("{method}: {other}"),
        }
    }

    fn register(&self, upload: &LayoutUpload) -> String {
        self.call("RegisterLayout", &(json(upload).as_str(),))
            .unwrap()
    }

    fn forget(&self, token: &str) -> bool {
        self.call("ForgetLayout", &(token,)).unwrap()
    }

    fn recognize(
        &self,
        trace: Trace,
        token: &str,
        max: u32,
        lang: &str,
    ) -> Result<Vec<(String, f64)>, ServiceError> {
        self.call("RecognizeSwipe", &(trace, token, max, lang))
    }

    fn complete(&self, word: &str, token: &str) -> Result<Vec<String>, ServiceError> {
        let rows: Vec<(String, f64)> = self.call(
            "CompleteWith",
            &(
                word,
                Vec::<String>::new(),
                6u32,
                "en_US",
                ("none", "none"),
                ("none", "none"),
                "prefer_matched",
                token,
                Vec::<(f64, f64)>::new(),
            ),
        )?;
        Ok(rows.into_iter().map(|(word, _)| word).collect())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let client = self.client.clone();
        drop(client);
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

#[test]
fn one_registered_token_serves_completion_and_swipe() {
    let (_temp, service) = Service::patricia(
        &[("hello", 200), ("help", 100), ("world", 150), ("cat", 180)],
        "en_US",
    );
    let upload = qwerty(&[], 1.0, (0.0, 0.0));
    let token = service.register(&upload);

    let swiped = service
        .recognize(trace(&upload, &["h", "e", "l", "o"]), &token, 6, "en_US")
        .unwrap();
    assert_eq!(words(&swiped).first(), Some(&"hello"), "{swiped:?}");
    assert!(
        service
            .complete("helo", &token)
            .unwrap()
            .contains(&"hello".to_string())
    );

    // Registering the same upload again is the same token: the geometry is
    // shared, not duplicated.
    assert_eq!(service.register(&upload), token);

    let unknown_swipe = service
        .recognize(trace(&upload, &["c", "a", "t"]), "deadbeef", 6, "en_US")
        .unwrap_err();
    let unknown_completion = service.complete("helo", "deadbeef").unwrap_err();
    assert!(unknown_swipe.invalid_args(), "{unknown_swipe}");
    assert!(
        unknown_swipe
            .message
            .contains("unknown layout token 'deadbeef'"),
        "{unknown_swipe}"
    );
    assert!(
        unknown_completion
            .message
            .contains("unknown layout token 'deadbeef'"),
        "{unknown_completion}"
    );
    let empty = service
        .recognize(trace(&upload, &["c", "a", "t"]), "", 6, "en_US")
        .unwrap_err();
    assert!(empty.invalid_args(), "{empty}");
    assert!(empty.message.contains("registered layout token"), "{empty}");
}

#[test]
fn accents_are_recognized_first_interior_and_last_through_alternates() {
    let (_temp, service) = Service::patricia(
        &[
            ("école", 200),
            ("ecole", 60),
            ("café", 200),
            ("cafe", 60),
            ("señor", 200),
            ("senor", 60),
            ("sensor", 90),
        ],
        "fr",
    );
    let upload = qwerty(&[("e", &["é", "è"]), ("n", &["ñ"])], 1.0, (0.0, 0.0));
    let token = service.register(&upload);

    for (path, expected) in [
        (vec!["e", "c", "o", "l", "e"], "école"),
        (vec!["c", "a", "f", "e"], "café"),
        (vec!["s", "e", "n", "o", "r"], "señor"),
    ] {
        let results = service
            .recognize(trace(&upload, &path), &token, 6, "fr")
            .unwrap();
        assert_eq!(
            words(&results).first(),
            Some(&expected),
            "{path:?}: {results:?}"
        );
    }
}

/// A Shift export that still recognizes unshifted entries: every key shows
/// its capital glyph as the main label with the unshifted twin beside it as
/// an alternate, exactly as such a layer is really drawn.
fn shift_layer(upload: &LayoutUpload) -> LayoutUpload {
    let keys = upload
        .keys
        .as_ref()
        .unwrap()
        .iter()
        .map(|key| KeyBox {
            label: key.label.to_uppercase(),
            alt_labels: {
                let mut alts: Vec<String> = key
                    .alt_labels
                    .iter()
                    .map(|alt| alt.to_uppercase())
                    .collect();
                alts.push(key.label.clone());
                alts.extend(key.alt_labels.iter().cloned());
                alts
            },
            ..key.clone()
        })
        .collect();
    LayoutUpload {
        keys: Some(keys),
        rows: None,
        ignored_labels: upload.ignored_labels.clone(),
    }
}

#[test]
fn dedicated_accent_keys_and_shift_layers_recognize_exactly_their_declared_labels() {
    let (_temp, service) = Service::patricia(
        &[
            ("été", 200),
            ("ete", 40),
            ("Paris", 220),
            ("paris", 40),
            ("part", 100),
            ("café", 200),
        ],
        "fr",
    );
    // A layout with its own accent keys, like the bottom row of an AZERTY
    // number layer, recognizes the accented word from those keys.
    let azerty: &[Row<'_>] = &[
        &[
            ("a", &[]),
            ("z", &[]),
            ("e", &[]),
            ("r", &[]),
            ("t", &[]),
            ("y", &[]),
        ],
        &[
            ("q", &[]),
            ("s", &[]),
            ("d", &[]),
            ("f", &[]),
            ("g", &[]),
            ("h", &[]),
        ],
        &[
            ("é", &[]),
            ("è", &[]),
            ("ç", &[]),
            ("à", &[]),
            ("ù", &[]),
            ("p", &[]),
        ],
    ];
    let upload = keyboard(azerty, 1.0, (0.0, 0.0));
    let token = service.register(&upload);
    let results = service
        .recognize(trace(&upload, &["é", "t", "é"]), &token, 6, "fr")
        .unwrap();
    assert_eq!(words(&results).first(), Some(&"été"), "{results:?}");

    // A Shift layer with both cases on every key: the capitalized entry is
    // returned as stored, and the lowercase entries still match through the
    // declared twins. Each stored spelling with a complete path is its own
    // candidate, the likelier one first.
    let shifted = shift_layer(&qwerty(&[("e", &["é"])], 1.0, (0.0, 0.0)));
    let shift_token = service.register(&shifted);
    let results = service
        .recognize(
            trace(&shifted, &["C", "A", "F", "E"]),
            &shift_token,
            6,
            "fr",
        )
        .unwrap();
    assert_eq!(words(&results).first(), Some(&"café"), "{results:?}");
    let results = service
        .recognize(
            trace(&shifted, &["P", "A", "R", "I", "S"]),
            &shift_token,
            6,
            "fr",
        )
        .unwrap();
    assert_eq!(words(&results).first(), Some(&"Paris"), "{results:?}");
    assert!(
        words(&results).contains(&"paris"),
        "both declared cases score as their own spellings: {results:?}"
    );

    // A capitals-only export declares no lowercase: a lowercase entry has no
    // complete path and is not invented onto the capital keys.
    let caps = uppercase(&qwerty(&[], 1.0, (0.0, 0.0)));
    let upper_only = service.register(&caps);
    let results = service
        .recognize(trace(&caps, &["C", "A", "F", "E"]), &upper_only, 6, "fr")
        .unwrap();
    assert!(
        results.is_empty(),
        "undeclared case relatives are refused: {results:?}"
    );
}

#[test]
fn composed_and_decomposed_forms_match_both_ways_and_keep_stored_spelling() {
    // A composed layout label and a decomposed stored word, whose combining
    // mark sits in a child trie node behind the shared prefix "cafe".
    let (_temp, service) =
        Service::patricia(&[("cafe\u{301}", 200), ("cafes", 100), ("cafe", 30)], "fr");
    let composed = qwerty(&[("e", &["\u{e9}"])], 1.0, (0.0, 0.0));
    let token = service.register(&composed);
    let results = service
        .recognize(trace(&composed, &["c", "a", "f", "e"]), &token, 6, "fr")
        .unwrap();
    assert_eq!(words(&results).first(), Some(&"cafe\u{301}"), "{results:?}");

    // A decomposed layout label and a composed stored word.
    let (_temp, service) = Service::patricia(&[("caf\u{e9}", 200), ("cafe", 30)], "fr");
    let decomposed = qwerty(&[("e", &["e\u{301}"])], 1.0, (0.0, 0.0));
    let token = service.register(&decomposed);
    let results = service
        .recognize(trace(&decomposed, &["c", "a", "f", "e"]), &token, 6, "fr")
        .unwrap();
    assert_eq!(words(&results).first(), Some(&"caf\u{e9}"), "{results:?}");
}

#[test]
fn non_latin_and_non_26_key_layouts_recognize_their_own_words() {
    let (_temp, service) = Service::patricia(
        &[("καλά", 200), ("ναι", 200), ("και", 180), ("κακά", 100)],
        "el",
    );
    let greek: &[Row<'_>] = &[
        &[("κ", &[]), ("α", &["ά"]), ("λ", &[]), ("ο", &["ό"])],
        &[("ν", &[]), ("ι", &["ί"]), ("σ", &["ς"])],
    ];
    let upload = keyboard(greek, 1.0, (0.0, 0.0));
    assert_eq!(upload.keys.as_ref().unwrap().len(), 7);
    let token = service.register(&upload);
    for (path, expected) in [
        (vec!["κ", "α", "λ", "α"], "καλά"),
        (vec!["ν", "α", "ι"], "ναι"),
        (vec!["κ", "α", "ι"], "και"),
    ] {
        let results = service
            .recognize(trace(&upload, &path), &token, 6, "el")
            .unwrap();
        assert_eq!(
            words(&results).first(),
            Some(&expected),
            "{path:?}: {results:?}"
        );
    }
}

#[test]
fn words_needing_unmapped_graphemes_are_never_offered() {
    let (_temp, service) = Service::patricia(
        &[
            ("naive", 100),
            ("naïve", 250),
            ("dont", 100),
            ("don't", 200),
        ],
        "en_US",
    );
    let plain = qwerty(&[], 1.0, (0.0, 0.0));
    let token = service.register(&plain);
    let results = service
        .recognize(
            trace(&plain, &["n", "a", "i", "v", "e"]),
            &token,
            6,
            "en_US",
        )
        .unwrap();
    assert_eq!(
        words(&results),
        vec!["naive"],
        "the likelier accented entry has no ï on this layout: {results:?}"
    );

    let with_diaeresis = qwerty(&[("i", &["ï"])], 1.0, (0.0, 0.0));
    let token = service.register(&with_diaeresis);
    let results = service
        .recognize(
            trace(&with_diaeresis, &["n", "a", "i", "v", "e"]),
            &token,
            6,
            "en_US",
        )
        .unwrap();
    assert_eq!(words(&results), vec!["naïve", "naive"], "{results:?}");

    // An apostrophe the client ignores is skipped in paths, never required.
    let mut ignoring = qwerty(&[], 1.0, (0.0, 0.0));
    ignoring.keys.as_mut().unwrap().push(KeyBox {
        label: "'".into(),
        alt_labels: Vec::new(),
        left: 300.0,
        top: 100.0,
        width: 36.0,
        height: 46.0,
    });
    ignoring.ignored_labels = vec!["'".into()];
    let token = service.register(&ignoring);
    let results = service
        .recognize(trace(&ignoring, &["d", "o", "n", "t"]), &token, 6, "en_US")
        .unwrap();
    assert_eq!(words(&results), vec!["don't", "dont"], "{results:?}");
    // Without that declaration the apostrophe is an unmapped grapheme.
    let plain_token = service.register(&plain);
    let results = service
        .recognize(
            trace(&plain, &["d", "o", "n", "t"]),
            &plain_token,
            6,
            "en_US",
        )
        .unwrap();
    assert_eq!(words(&results), vec!["dont"], "{results:?}");
}

#[test]
fn translated_scaled_and_fractional_layouts_recognize_the_same_path() {
    let (_temp, service) = Service::patricia(
        &[("cat", 180), ("cart", 160), ("cut", 200), ("dog", 150)],
        "en_US",
    );
    let mut rankings = Vec::new();
    for (scale, offset) in [
        (1.0, (0.0, 0.0)),
        (1.75, (100.0, 80.0)),
        (0.93, (0.37, 0.61)),
    ] {
        let upload = qwerty(&[], scale, offset);
        let token = service.register(&upload);
        let results = service
            .recognize(trace(&upload, &["c", "a", "t"]), &token, 6, "en_US")
            .unwrap();
        assert_eq!(
            words(&results).first(),
            Some(&"cat"),
            "{scale} {offset:?}: {results:?}"
        );
        rankings.push(
            words(&results)
                .iter()
                .map(|w| w.to_string())
                .collect::<Vec<_>>(),
        );
    }
    assert!(
        rankings.windows(2).all(|pair| pair[0] == pair[1]),
        "{rankings:?}"
    );
}

#[test]
fn stationary_collapsed_and_malformed_traces_are_rejected() {
    let (_temp, service) = Service::patricia(&[("cat", 180)], "en_US");
    let upload = qwerty(&[], 1.0, (0.0, 0.0));
    let token = service.register(&upload);
    let good = trace(&upload, &["c", "a", "t"]);
    let (x, y) = center(&upload, "c");
    let cases: Vec<(Trace, &str)> = vec![
        (vec![good[0]], "2..512"),
        (vec![good[0]; 513], "2..512"),
        (vec![(x, y, 0), (x, y, 10), (x, y, 20)], "no usable motion"),
        (
            vec![(x, y, 0), (x + 0.4, y, 10), (x, y, 20)],
            "no usable motion",
        ),
        (vec![(f64::NAN, y, 0), good[1]], "coordinates"),
        (vec![(x, y, 0), (x + 40.0, y, 10001)], "timestamps"),
        (vec![(x, y, 5), (x + 40.0, y, 10)], "start at zero"),
    ];
    for (bad, fragment) in cases {
        let error = service.recognize(bad, &token, 6, "en_US").unwrap_err();
        assert!(error.invalid_args(), "{error}");
        assert!(error.message.contains(fragment), "{fragment}: {error}");
    }
    let results = service.recognize(good, &token, 6, "en_US").unwrap();
    assert_eq!(words(&results).first(), Some(&"cat"));
}

#[test]
fn forgotten_and_evicted_tokens_fail_like_completion() {
    let (_temp, service) = Service::patricia(&[("cat", 180)], "en_US");
    let upload = qwerty(&[], 1.0, (0.0, 0.0));
    let token = service.register(&upload);
    assert!(service.forget(&token));
    let error = service
        .recognize(trace(&upload, &["c", "a", "t"]), &token, 6, "en_US")
        .unwrap_err();
    assert!(error.message.contains("unknown layout token"), "{error}");
    let error = service.complete("cat", &token).unwrap_err();
    assert!(error.message.contains("unknown layout token"), "{error}");

    // The registry keeps sixteen layouts; the oldest is evicted by the seventeenth.
    let token = service.register(&upload);
    for index in 0..16 {
        service.register(&qwerty(&[], 1.0, (index as f64 + 1.0, 0.0)));
    }
    let error = service
        .recognize(trace(&upload, &["c", "a", "t"]), &token, 6, "en_US")
        .unwrap_err();
    assert!(error.message.contains("unknown layout token"), "{error}");
    let error = service.complete("cat", &token).unwrap_err();
    assert!(error.message.contains("unknown layout token"), "{error}");
}

/// A Patricia backend whose candidate search waits until the test lets it go,
/// so a token can be forgotten while a recognition holds the layout.
struct GatedPatricia {
    inner: PatriciaDictionaryBackend,
    entered: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

impl DictionaryBackend for GatedPatricia {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult> {
        self.inner.query_prefixes(queries)
    }
    fn get_frequency(&self, word: &str) -> f64 {
        self.inner.get_frequency(word)
    }
    fn contains(&self, word: &str) -> bool {
        self.inner.contains(word)
    }
    fn swipe_candidates(
        &self,
        vocabulary: &SwipeVocabulary,
        starts: &[String],
        ends: &[String],
        deadline: Instant,
    ) -> Result<Vec<SwipeCandidate>, String> {
        self.entered.store(true, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.inner
            .swipe_candidates(vocabulary, starts, ends, deadline)
    }
}

#[test]
fn forgetting_a_token_after_capture_leaves_the_request_intact() {
    let temp = tempfile::tempdir().unwrap();
    let path = dictionary(temp.path(), "en_US", &[("cat", 180), ("cut", 100)]);
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let gated = GatedPatricia {
        inner: PatriciaDictionaryBackend::open(&path).unwrap(),
        entered: entered.clone(),
        release: release.clone(),
    };
    let service = Arc::new(Service::start(DaemonHandler::new(
        Box::new(gated),
        None,
        None,
        "en_US".into(),
    )));
    let upload = qwerty(&[], 1.0, (0.0, 0.0));
    let token = service.register(&upload);

    let worker_service = service.clone();
    let worker_token = token.clone();
    let worker_trace = trace(&upload, &["c", "a", "t"]);
    let worker = std::thread::spawn(move || {
        worker_service.recognize(worker_trace, &worker_token, 6, "en_US")
    });
    let started = Instant::now();
    while !entered.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "recognition never captured its layout"
        );
        std::thread::sleep(Duration::from_millis(2));
    }

    // The token disappears while the request is running on the captured layout.
    assert!(service.forget(&token));
    release.store(true, Ordering::SeqCst);
    let results = worker.join().unwrap().unwrap();
    assert_eq!(words(&results).first(), Some(&"cat"), "{results:?}");

    let error = service
        .recognize(trace(&upload, &["c", "a", "t"]), &token, 6, "en_US")
        .unwrap_err();
    assert!(error.message.contains("unknown layout token"), "{error}");
}

#[test]
fn caller_max_and_the_service_cap_are_honoured() {
    let (_temp, service) = Service::patricia(
        &[("cat", 180), ("cart", 160), ("cut", 200), ("cot", 100)],
        "en_US",
    );
    let upload = qwerty(&[], 1.0, (0.0, 0.0));
    let token = service.register(&upload);
    let path = trace(&upload, &["c", "a", "t"]);
    assert!(
        service
            .recognize(path.clone(), &token, 0, "en_US")
            .unwrap()
            .is_empty()
    );
    let one = service.recognize(path.clone(), &token, 1, "en_US").unwrap();
    let six = service.recognize(path.clone(), &token, 6, "en_US").unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0], six[0]);
    assert!(six.len() >= 2, "{six:?}");
    let error = service.recognize(path, &token, 1001, "en_US").unwrap_err();
    assert!(error.message.contains("exceeds swipe cap"), "{error}");
}

/// A request for no results is answered at once even while every recognition
/// worker is occupied: it resolves no token, validates no trace and takes no
/// permit, while a request that does want results is still refused as busy.
#[test]
fn zero_max_returns_empty_immediately_under_saturated_workers() {
    let temp = tempfile::tempdir().unwrap();
    let path = dictionary(temp.path(), "en_US", &[("cat", 180), ("cut", 100)]);
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let gated = GatedPatricia {
        inner: PatriciaDictionaryBackend::open(&path).unwrap(),
        entered: entered.clone(),
        release: release.clone(),
    };
    let handler =
        DaemonHandler::new(Box::new(gated), None, None, "en_US".into()).with_swipe_workers(1);
    let service = Arc::new(Service::start(handler));
    let upload = qwerty(&[], 1.0, (0.0, 0.0));
    let token = service.register(&upload);

    let occupant_service = service.clone();
    let occupant_token = token.clone();
    let occupant_trace = trace(&upload, &["c", "a", "t"]);
    let occupant = std::thread::spawn(move || {
        occupant_service.recognize(occupant_trace, &occupant_token, 6, "en_US")
    });
    let started = Instant::now();
    while !entered.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the occupant never entered candidate search"
        );
        std::thread::sleep(Duration::from_millis(2));
    }

    // The only worker is busy. Asking for nothing still gets an immediate,
    // empty answer, whatever the token or trace.
    let answered = Instant::now();
    assert_eq!(
        service
            .recognize(trace(&upload, &["c", "a", "t"]), &token, 0, "en_US")
            .unwrap(),
        Vec::new()
    );
    assert_eq!(
        service
            .recognize(trace(&upload, &["c", "a", "t"]), "deadbeef", 0, "en_US")
            .unwrap(),
        Vec::new()
    );
    assert_eq!(
        service.recognize(Vec::new(), &token, 0, "en_US").unwrap(),
        Vec::new()
    );
    assert!(
        answered.elapsed() < Duration::from_millis(500),
        "empty answers waited on the busy worker: {:?}",
        answered.elapsed()
    );
    // Asking for results is still backpressure, not a queue.
    let busy = service
        .recognize(trace(&upload, &["c", "a", "t"]), &token, 1, "en_US")
        .unwrap_err();
    assert!(busy.message.contains("busy"), "{busy}");

    release.store(true, Ordering::SeqCst);
    let results = occupant.join().unwrap().unwrap();
    assert_eq!(words(&results).first(), Some(&"cat"), "{results:?}");
}

#[test]
fn full_language_routing_is_unchanged_for_swipes() {
    let temp = tempfile::tempdir().unwrap();
    dictionary(temp.path(), "fr_FR", &[("élan", 200)]);
    dictionary(temp.path(), "fr", &[("clan", 200)]);
    let mut config = DaemonConfig::default_for("fr_FR-br");
    config.backend_chain = "patricia".into();
    config.language_paths.system_dir = temp.path().to_path_buf();
    let service = Service::start(DaemonHandler::with_config(config));
    let upload = qwerty(&[("e", &["é"])], 1.0, (0.0, 0.0));
    let token = service.register(&upload);

    // The full tag falls back to its region: fr_FR-br -> fr_FR.
    let results = service
        .recognize(trace(&upload, &["e", "l", "a", "n"]), &token, 6, "fr_FR-br")
        .unwrap();
    assert_eq!(words(&results), vec!["élan"], "{results:?}");
    // A bare language stays bare and is never upgraded to a region.
    let results = service
        .recognize(trace(&upload, &["c", "l", "a", "n"]), &token, 6, "fr")
        .unwrap();
    assert_eq!(words(&results), vec!["clan"], "{results:?}");
    let results = service
        .recognize(trace(&upload, &["e", "l", "a", "n"]), &token, 6, "fr")
        .unwrap();
    assert!(
        results.is_empty(),
        "the base dictionary has no élan: {results:?}"
    );
    // A missing dictionary is an error, never an English substitute.
    let missing = service
        .recognize(trace(&upload, &["e", "l", "a", "n"]), &token, 6, "zz_ZZ")
        .unwrap_err();
    assert!(
        missing.message.contains("no dictionary loaded for 'zz_ZZ'"),
        "{missing}"
    );
    let invalid = service
        .recognize(trace(&upload, &["e", "l", "a", "n"]), &token, 6, "../fr")
        .unwrap_err();
    assert!(
        invalid.message.contains("invalid language tag"),
        "{invalid}"
    );
}

/// A gesture over the real exported US normal layer: through the period key's
/// centre and on to `t` returns the apostrophe word, exactly as stored. The
/// intent is that the punctuation is the period key's apostrophe alternate,
/// not a synthetic approximation of it.
#[test]
fn exported_us_normal_swipe_traces_the_period_apostrophe_alternative() {
    let (_temp, service) = Service::patricia(&[("don't", 220), ("dont", 70), ("dot", 90)], "en_US");
    let upload = exported_us_normal();
    let token = service.register(&upload);

    // d -> o -> n -> period -> t: the same key rectangles the client exported.
    let results = service
        .recognize(
            trace(&upload, &["d", "o", "n", ".", "t"]),
            &token,
            6,
            "en_US",
        )
        .unwrap();
    assert_eq!(
        results.first().map(|(word, _)| word.as_str()),
        Some("don't"),
        "the intended word is first, in its stored spelling: {results:?}"
    );
    assert!(
        results
            .iter()
            .all(|(word, score)| !word.is_empty() && score.is_finite()),
        "{results:?}"
    );

    // Control: the same rectangles and trace with the apostrophe removed from
    // the period key's long-press menu. The apostrophe word now needs an
    // unmapped grapheme, so the explicit punctuation trace is what reached it
    // above. The layout is otherwise intact: the apostrophe-free entry still
    // recognizes over its own path.
    let plain = exported_us_normal_without_apostrophe();
    let plain_token = service.register(&plain);
    let results = service
        .recognize(
            trace(&plain, &["d", "o", "n", ".", "t"]),
            &plain_token,
            6,
            "en_US",
        )
        .unwrap();
    assert!(
        !words(&results).contains(&"don't"),
        "without an apostrophe key the word has no complete path: {results:?}"
    );
    let results = service
        .recognize(
            trace(&plain, &["d", "o", "n", "t"]),
            &plain_token,
            6,
            "en_US",
        )
        .unwrap();
    assert_eq!(
        words(&results).first(),
        Some(&"dont"),
        "the control layout still recognizes its own words: {results:?}"
    );
}

/// The prototype's key-array body `(a(ddu) a(sdddd) u s)` is refused, not
/// recognized against some default geometry.
#[test]
fn the_prototype_key_array_body_is_refused() {
    let (_temp, service) = Service::patricia(&[("cat", 180)], "en_US");
    let upload = qwerty(&[], 1.0, (0.0, 0.0));
    let keys: Vec<(String, f64, f64, f64, f64)> = upload
        .keys
        .as_ref()
        .unwrap()
        .iter()
        .map(|key| {
            (
                key.label.clone(),
                f64::from(key.left),
                f64::from(key.top),
                f64::from(key.width),
                f64::from(key.height),
            )
        })
        .collect();
    let legacy: Result<Vec<(String, f64)>, ServiceError> = service.call(
        "RecognizeSwipe",
        &(trace(&upload, &["c", "a", "t"]), keys, 6u32, "en_US"),
    );
    let error = legacy.unwrap_err();
    assert!(
        error.invalid_args() || error.name.contains("Error"),
        "{error}"
    );
}
