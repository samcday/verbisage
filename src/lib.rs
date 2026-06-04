pub mod daemon;
pub mod dictionary;
pub mod prediction;
pub mod spellcheck;

pub use daemon::{DaemonHandler, DaemonRequest, DaemonResponse};
pub use dictionary::{
    CompactDictionary, DictionaryBackend, DictionaryQuery, DictionaryResult, FileDictionaryBackend,
    SharedQueryCache,
};
pub use prediction::{Prediction, Predictor};
pub use spellcheck::SpellChecker;

#[cfg(feature = "sqlite")]
pub use dictionary::SqliteDictionaryBackend;
