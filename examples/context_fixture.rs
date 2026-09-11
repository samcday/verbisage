//! Known-data fixture for Stevia's isolated native interaction tests.
//! cargo run --all-features --example context_fixture -- /new/output/directory
#[cfg(feature = "patricia")]
fn main() {
    let path = std::path::PathBuf::from(std::env::args_os().nth(1).expect("output path"));
    assert!(!path.exists(), "refusing to replace existing data");
    let mut dict = patricia_dict::Dictionary::create_empty_v403(path, "en_US").unwrap();
    for (word, probability) in [
        ("see", 100),
        ("you", 100),
        ("later", 10),
        ("large", 230),
        ("last", 220),
        ("hello", 200),
        ("help", 100),
    ] {
        dict.append(word, probability).unwrap();
    }
    for (context, target) in [("see", "you"), ("hello", "you"), ("you", "later")] {
        dict.add_ngram(target, &[context], 250).unwrap();
    }
    dict.add_ngram("later", &["see", "you"], 250).unwrap();
    dict.add_ngram("later", &["hello", "you"], 250).unwrap();
}

#[cfg(not(feature = "patricia"))]
fn main() {
    panic!("context_fixture needs the patricia feature");
}
