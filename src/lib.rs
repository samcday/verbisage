pub mod backends;
pub mod cli;
pub mod clients;
pub mod config;
pub mod daemon;
pub mod debug;
pub mod dictionary;
pub mod prediction;
pub mod spellcheck;

pub use clients::{ClientError, StdioClient};
pub use daemon::{DaemonHandler, DaemonRequest, DaemonResponse};
pub use dictionary::{
    CompactDictionary, DictionaryBackend, DictionaryQuery, DictionaryResult, FileDictionaryBackend,
    SharedQueryCache,
};
pub use prediction::{Prediction, Predictor};
pub use spellcheck::SpellChecker;

#[cfg(feature = "sqlite")]
pub use dictionary::SqliteDictionaryBackend;

#[cfg(feature = "dbus")]
pub use clients::DbusClient;
