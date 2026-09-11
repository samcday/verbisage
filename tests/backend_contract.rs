use verbisage::{DictionaryBackend, DictionaryQuery, FileDictionaryBackend};

fn prefix(s: &str) -> DictionaryQuery {
    DictionaryQuery {
        prefix: Some(s.into()),
        suffix: None,
        min_length: None,
        max_length: None,
    }
}

#[test]
fn exact_unicode_matching_and_native_count_normalization() {
    let dict = FileDictionaryBackend::new();
    dict.add_word("Cafe\u{301}", 30.0, false).unwrap();
    dict.add_word("café", 10.0, false).unwrap();
    assert!(dict.contains("Café"));
    assert!(!dict.contains("Cafe\u{301}"));
    assert!(!dict.contains("CAFÉ"));
    assert_eq!(dict.get_frequency("Café"), 0.75);
    assert_eq!(dict.query_limited(&[prefix("C")], 10)[0].word, "Café");
    assert_eq!(dict.query_limited(&[prefix("c")], 10)[0].word, "café");
    let mut q = prefix("C");
    q.min_length = Some(4);
    q.max_length = Some(4);
    assert_eq!(dict.query_limited(&[q], 10).len(), 1);
    dict.add_word("Café", 10.0, true).unwrap();
    assert_eq!(dict.get_frequency("Café"), 0.5);
    assert_eq!(dict.get_frequency("missing"), -1.0);
}

#[test]
fn malformed_frequencies_sort_after_finite_scores_with_lexical_ties() {
    use verbisage::dictionary::{DictionaryResult, rank_results};
    let mut rows = vec![
        DictionaryResult {
            word: "z".into(),
            confidence: f64::NAN,
        },
        DictionaryResult {
            word: "b".into(),
            confidence: 0.2,
        },
        DictionaryResult {
            word: "a".into(),
            confidence: 0.2,
        },
        DictionaryResult {
            word: "x".into(),
            confidence: f64::INFINITY,
        },
    ];
    rows.sort_by(rank_results);
    assert_eq!(
        rows.iter().map(|r| r.word.as_str()).collect::<Vec<_>>(),
        ["a", "b", "x", "z"]
    );
    let mut dict = FileDictionaryBackend::new();
    dict.add_word_mut("bad".into(), f64::NAN);
    dict.add_word_mut("good".into(), 10.0);
    assert_eq!(dict.get_frequency("good"), 1.0);
    assert_eq!(dict.get_frequency("bad"), -1.0);
}

#[test]
fn text_store_ingestion_deduplicates_canonically_equivalent_words() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("words.txt");
    std::fs::write(&path, "Cafe\u{301} 30\nCafé 10\nother 10\n").unwrap();
    let file = FileDictionaryBackend::from_frequency_file(&path, false).unwrap();
    let compact = verbisage::CompactDictionary::from_frequency_file(&path).unwrap();
    for dict in [&file as &dyn DictionaryBackend, &compact] {
        assert_eq!(dict.get_frequency("Café"), 0.5);
        assert_eq!(dict.query_prefixes(&[prefix("C")]).len(), 1);
    }
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_exact_literals_normalization_and_cross_connection_updates() {
    use verbisage::PresageSqliteBackend;
    use verbisage::prediction::ngram_backend::NgramBackend;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("words.db");
    let db = PresageSqliteBackend::open(&path, true).unwrap();
    for (word, count) in [
        ("Cafe\u{301}", 30.0),
        ("café", 10.0),
        ("a*b", 5.0),
        ("a?b", 5.0),
        ("a[b", 5.0),
        ("a%b", 5.0),
    ] {
        db.add_word(word, count, false).unwrap();
    }
    assert_eq!(db.get_frequency("Café"), 0.5);
    assert!(!db.contains("CAFÉ"));
    assert_eq!(db.query_prefixes(&[prefix("C")])[0].word, "Café");
    for word in ["a*b", "a?b", "a[b", "a%b"] {
        let rows = db.query_limited(&[prefix(word)], 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].word, word);
    }
    db.enable_cache(2);
    assert_eq!(db.query_prefixes(&[prefix("C")])[0].confidence, 0.5);
    let other = rusqlite::Connection::open(&path).unwrap();
    other
        .execute("UPDATE _1_gram SET count=90 WHERE word='Café'", [])
        .unwrap();
    assert_eq!(db.get_frequency("Café"), 0.75);
    assert_eq!(db.query_prefixes(&[prefix("C")])[0].confidence, 0.75);
    db.increase_ngram_frequency(&["Café"], 0.6, false).unwrap();
    assert_eq!(db.ngram_count(&["Café"]), 91);
}
