use verbisage::completion::{
    AndroidCompleter, CompletionConfig, CompletionEngine, CompletionInput,
};
use verbisage::dictionary::{DictionaryBackend, FileDictionaryBackend};
use verbisage::text::{BOS, CaseFold, CasePreference, Normalization, TextPrep};

#[cfg(feature = "patricia")]
#[test]
fn daemon_uses_explicit_patricia_path_and_respects_skip() {
    use verbisage::daemon::{DaemonConfig, DaemonHandler};
    use verbisage::dictionary::paths::PathOverride;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("custom.dict");
    let mut native = patricia_dict::Dictionary::create_empty_v403(&path, "en_US").unwrap();
    native.append("fixtureword", 200).unwrap();
    drop(native);
    let mut config = DaemonConfig::default_for("en_US");
    config.backend_chain = "patricia".into();
    config.language_paths.system_file_override = PathOverride::File(path);
    let handler = DaemonHandler::with_config(config);
    let result = handler
        .complete_with(&CompletionInput::default(), 6, "en_US")
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].word, "fixtureword");
    let mut config = DaemonConfig::default_for("en_US");
    config.backend_chain = "patricia".into();
    config.language_paths.system_file_override = PathOverride::Skip;
    let handler = DaemonHandler::with_config(config);
    assert!(
        handler
            .complete_with(&CompletionInput::default(), 6, "en_US")
            .is_err()
    );
}

#[cfg(feature = "patricia")]
#[test]
fn native_sentence_start_resolves_integer_sentinel_and_missing_data_backs_off() {
    use std::sync::Arc;
    use verbisage::dictionary::patricia::PatriciaDictionaryBackend;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bos");
    let mut native = patricia_dict::Dictionary::create_empty_v403(&path, "en_US").unwrap();
    native.append("hello", 20).unwrap();
    native.append("world", 240).unwrap();
    // The writer accepts Unicode words. Build a three-byte placeholder and
    // replace that single trie code point with Android's integer-only marker.
    // This tests native addressing rather than storing literal U+FFFF rows.
    native.append("\u{10ffff}", 100).unwrap();
    native.add_ngram("hello", &["\u{10ffff}"], 250).unwrap();
    drop(native);
    let body = path.join("bos.body");
    let mut bytes = std::fs::read(&body).unwrap();
    let trie_len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    let matches: Vec<_> = bytes[4..4 + trie_len]
        .windows(3)
        .enumerate()
        .filter(|(_, w)| *w == [0x10, 0xff, 0xff])
        .map(|(i, _)| i + 4)
        .collect();
    assert_eq!(matches.len(), 1);
    bytes[matches[0]..matches[0] + 3].copy_from_slice(&[0x11, 0, 0]);
    std::fs::write(body, bytes).unwrap();
    let native = patricia_dict::Dictionary::open(&path).unwrap();
    let context = native.prepare_context(&[BOS]);
    assert_eq!(context.available_order(), 2);
    assert_eq!(
        native
            .ngram_scores_prepared("hello", &context)
            .unwrap()
            .bigram,
        Some(250)
    );
    let backend = Arc::new(PatriciaDictionaryBackend::open(&path).unwrap());
    let engine = AndroidCompleter::new(backend.as_ref()).with_predictor(Some(&backend));
    assert_eq!(engine.complete(None, 6)[0].word, "world");
    let rows = engine
        .complete_with(
            &CompletionInput {
                context: &["old", "<s>"],
                ..Default::default()
            },
            6,
        )
        .unwrap();
    assert_eq!(rows[0].word, "hello");
    assert_eq!(rows.len(), 2);
    assert!(
        words(&rows)
            .iter()
            .all(|word| !word.is_empty() && *word != BOS)
    );

    let path = temp.path().join("without-bos");
    let mut native = patricia_dict::Dictionary::create_empty_v403(&path, "en_US").unwrap();
    native.append("hello", 20).unwrap();
    native.append("world", 240).unwrap();
    native.add_ngram("hello", &["world"], 250).unwrap();
    drop(native);
    let backend = Arc::new(PatriciaDictionaryBackend::open(&path).unwrap());
    let engine = AndroidCompleter::new(backend.as_ref()).with_predictor(Some(&backend));
    assert_eq!(
        engine.complete(None, 6),
        engine
            .complete_with(
                &CompletionInput {
                    context: &["<s>"],
                    ..Default::default()
                },
                6
            )
            .unwrap()
    );
    let request = |context| {
        engine
            .complete_with(
                &CompletionInput {
                    context,
                    ..Default::default()
                },
                6,
            )
            .unwrap()
    };
    assert_eq!(request(&["world"]), request(&["<s>", "world"]));
}

fn dictionary(rows: &[(&str, f64)]) -> FileDictionaryBackend {
    let mut dictionary = FileDictionaryBackend::new();
    for &(word, count) in rows {
        dictionary.add_word_mut(word.into(), count);
    }
    dictionary
}
fn words(rows: &[verbisage::completion::CompletionCandidate]) -> Vec<&str> {
    rows.iter().map(|r| r.word.as_str()).collect()
}
#[test]
fn typo_prefix_known_short_and_unknown_frequency_policies() {
    let dict = dictionary(&[
        ("held", 158.0),
        ("help", 149.0),
        ("hero", 126.0),
        ("hello", 120.0),
        ("helots", 65.0),
        ("helot", 52.0),
    ]);
    let engine = AndroidCompleter::new(&dict);
    let rows = engine
        .complete_with(
            &CompletionInput {
                input: "helo",
                ..Default::default()
            },
            6,
        )
        .unwrap();
    assert_eq!(rows[0].word, "hello");
    assert!(!rows[0].is_exact);
    assert!(rows.iter().any(|r| r.word == "helots" && r.is_exact));
    let dict = dictionary(&[
        ("test", 100.0),
        ("testing", 80.0),
        ("best", 900.0),
        ("he", 80.0),
        ("help", 70.0),
        ("the", 1000.0),
    ]);
    let engine = AndroidCompleter::new(&dict);
    assert!(!words(&engine.complete(Some("test"), 6)).contains(&"best"));
    assert!(!words(&engine.complete(Some("he"), 6)).contains(&"the"));
    let dict = dictionary(&[
        ("beta", f64::NAN),
        ("alpha", f64::INFINITY),
        ("gamma", -3.0),
    ]);
    let rows = AndroidCompleter::new(&dict).complete(None, 6);
    assert_eq!(words(&rows), ["alpha", "beta", "gamma"]);
    assert!(
        rows.iter()
            .all(|r| r.score.is_finite() && (0.0..=1.0).contains(&r.score))
    );
}
#[test]
fn exact_defaults_explicit_full_fold_and_stored_spelling() {
    let dict = dictionary(&[
        ("Straße", 1.0),
        ("STRASSE", 1.0),
        ("Élan", 1.0),
        (BOS, 1000.0),
    ]);
    let engine = AndroidCompleter::new(&dict);
    assert!(engine.complete(Some("stras"), 6).is_empty());
    let prep = TextPrep {
        normalization: Normalization::Nfc,
        fold: CaseFold::Full,
    };
    let rows = engine
        .complete_with(
            &CompletionInput {
                input: "stras",
                input_prep: prep,
                ..Default::default()
            },
            6,
        )
        .unwrap();
    assert_eq!(words(&rows), ["STRASSE", "Straße"]);
    let rows = engine
        .complete_with(
            &CompletionInput {
                input: "E\u{301}l",
                input_prep: prep,
                ..Default::default()
            },
            6,
        )
        .unwrap();
    assert_eq!(words(&rows), ["Élan"]);
    assert!(!words(&engine.complete(None, 6)).contains(&BOS));
}
#[test]
fn case_bonus_is_input_only_and_can_be_disabled_for_autocaps() {
    let dict = dictionary(&[("HELLO", 1.0), ("Hello", 1.0)]);
    let engine = AndroidCompleter::new(&dict);
    let mut input = CompletionInput {
        input: "Hel",
        input_prep: TextPrep::CLI_DEFAULT,
        ..Default::default()
    };
    assert_eq!(engine.complete_with(&input, 2).unwrap()[0].word, "Hello");
    input.case_preference = CasePreference::Insensitive;
    assert_eq!(engine.complete_with(&input, 2).unwrap()[0].word, "HELLO");
    input.context = &["OLD", "CONTEXT"];
    assert_eq!(engine.complete_with(&input, 2).unwrap()[0].word, "HELLO");
}
#[test]
fn max_is_not_clamped_and_search_exhaustion_is_an_error() {
    let mut dict = FileDictionaryBackend::new();
    for i in 0..250 {
        dict.add_word_mut(format!("word{i:03}"), 1.0);
    }
    let engine = AndroidCompleter::new(&dict);
    assert_eq!(engine.complete(None, 150).len(), 150);
    assert_eq!(
        verbisage::completion::complete(&dict, "word", 150).len(),
        150
    );
    assert_eq!(
        engine.complete(None, 200),
        engine.complete(None, 250)[..200]
    );
    let input = CompletionInput {
        input: "z",
        ..Default::default()
    };
    let engine = AndroidCompleter::new(&dict).with_config(CompletionConfig {
        search_budget: std::time::Duration::ZERO,
        ..Default::default()
    });
    assert!(
        engine
            .complete_with(&input, 6)
            .unwrap_err()
            .contains("time budget")
    );
    assert!(engine.complete_with(&input, 0).unwrap().is_empty());
    let engine = AndroidCompleter::new(&dict).with_config(CompletionConfig {
        max_search_candidates: 100,
        ..Default::default()
    });
    assert!(
        engine
            .complete_with(&CompletionInput::default(), 6)
            .unwrap_err()
            .contains("word budget")
    );
}
#[cfg(feature = "sqlite")]
#[test]
fn sqlite_contextual_prediction_prefix_chain_and_bos_use_one_ranking() {
    use std::sync::Arc;
    use verbisage::dictionary::PresageSqliteBackend;
    use verbisage::prediction::{Predictor, smoothed::SmoothedPredictor};
    let dict = Arc::new(PresageSqliteBackend::new());
    for (word, count) in [
        ("see", 100.0),
        ("you", 100.0),
        ("later", 5.0),
        ("large", 500.0),
        ("last", 400.0),
        (BOS, 100.0),
    ] {
        dict.add_word(word, count, true).unwrap();
    }
    let predictor = SmoothedPredictor::new(dict.clone());
    for (ngram, count) in [
        (vec!["see", "you"], 90.0),
        (vec!["you", "later"], 90.0),
        (vec!["see", "you", "later"], 85.0),
        (vec![BOS, "see"], 90.0),
    ] {
        predictor
            .increase_ngram_frequency(&ngram, count, true)
            .unwrap();
    }
    let engine = AndroidCompleter::new(dict.as_ref()).with_predictor(Some(&predictor));
    assert_eq!(
        engine
            .complete_with(
                &CompletionInput {
                    context: &["see"],
                    ..Default::default()
                },
                1
            )
            .unwrap()[0]
            .word,
        "you"
    );
    let context = &["see", "you"];
    let predicted = engine
        .complete_with(
            &CompletionInput {
                context,
                ..Default::default()
            },
            6,
        )
        .unwrap();
    let completed = engine
        .complete_with(
            &CompletionInput {
                input: "l",
                context,
                ..Default::default()
            },
            6,
        )
        .unwrap();
    assert_eq!(predicted[0].word, "later");
    assert_eq!(completed[0], predicted[0]);
    let sentence = engine
        .complete_with(
            &CompletionInput {
                context: &["old", "<s>"],
                ..Default::default()
            },
            6,
        )
        .unwrap();
    assert_eq!(sentence[0].word, "see");
    assert!(!words(&sentence).contains(&BOS));
    assert_eq!(
        engine
            .complete_with(
                &CompletionInput {
                    context: &["unknown"],
                    ..Default::default()
                },
                6
            )
            .unwrap(),
        engine.complete(None, 6)
    );
}
#[cfg(feature = "patricia")]
#[test]
fn patricia_probability_context_uses_same_ranking_and_search_keeps_rare_matches() {
    use std::sync::Arc;
    use verbisage::dictionary::patricia::PatriciaDictionaryBackend;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("words.dict");
    let mut native = patricia_dict::Dictionary::create_empty_v403(&path, "en_US").unwrap();
    for (word, p) in [
        ("see", 100),
        ("you", 100),
        ("later", 10),
        ("large", 230),
        ("last", 220),
        ("London", 240),
    ] {
        native.append(word, p).unwrap();
    }
    native.add_ngram("you", &["see"], 250).unwrap();
    native.add_ngram("later", &["you"], 250).unwrap();
    drop(native);
    let dict = Arc::new(PatriciaDictionaryBackend::open(&path).unwrap());
    let engine = AndroidCompleter::new(dict.as_ref()).with_predictor(Some(&dict));
    let input = CompletionInput {
        input: "l",
        context: &["see", "you"],
        input_prep: TextPrep::CLI_DEFAULT,
        context_prep: TextPrep::CLI_DEFAULT,
        ..Default::default()
    };
    let completion = engine.complete_with(&input, 1).unwrap();
    assert_eq!(completion[0].word, "later");
    let prediction = engine
        .complete_with(
            &CompletionInput {
                context: &["see", "you"],
                ..Default::default()
            },
            1,
        )
        .unwrap();
    assert_eq!(prediction, completion);
    assert_eq!(
        engine
            .complete_with(
                &CompletionInput {
                    context: &["see"],
                    ..Default::default()
                },
                1
            )
            .unwrap()[0]
            .word,
        "you"
    );
    assert_eq!(
        engine
            .complete_with(
                &CompletionInput {
                    context: &["unknown"],
                    ..Default::default()
                },
                6
            )
            .unwrap(),
        engine.complete(None, 6)
    );
}
