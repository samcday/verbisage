use zbus::interface;

use crate::dictionary::DictionaryQuery;

use super::DaemonHandler;

/// D-Bus interface exposing dictionary, spell-check, and prediction methods.
///
/// Registered at the well-known name `org.verbisage.Dictionary`, object path
/// `/org/verbisage/Dictionary`, interface `org.verbisage.Dictionary1`.
pub struct VerbisageDbus {
    handler: DaemonHandler,
}

impl VerbisageDbus {
    pub fn new(handler: DaemonHandler) -> Self {
        Self { handler }
    }
}

#[interface(name = "org.verbisage.Dictionary1", introspection_docs = true)]
impl VerbisageDbus {
    /// Check whether a single word is recognised by the dictionary for the
    /// given language.
    ///
    /// @param word  Word to check.
    /// @param lang  BCP-47 / POSIX language tag (e.g. "en_US").
    /// @return      `true` if @word is recognised, `false` otherwise.
    #[zbus(out_args("result"))]
    async fn is_correct(&self, word: &str, lang: &str) -> bool {
        crate::veprintln!("[dbus-server] IsCorrect({}, {})", word, lang);
        self.handler.is_correct(word, lang)
    }

    /// Return spelling suggestions (corrections) for a given word in the
    /// given language.
    ///
    /// @param word  Word to correct.
    /// @param max   Maximum number of suggestions to return.
    /// @param lang  Language tag.
    /// @return      Ordered list of spelling suggestions (may be empty).
    #[zbus(out_args("result"))]
    async fn suggest(&self, word: &str, max: u32, lang: &str) -> Vec<String> {
        crate::veprintln!("[dbus-server] Suggest({}, {}, {})", word, max, lang);
        self.handler.suggest(word, max as usize, lang)
    }

    /// Search the dictionary for words matching prefix and/or suffix
    /// constraints.  When both `prefixes` and `suffixes` are empty every
    /// entry is a candidate (subject to length bounds).
    ///
    /// @param prefixes  Required prefixes (empty = no prefix constraint).
    /// @param suffixes  Required suffixes (empty = no suffix constraint).
    /// @param min_len   Minimum word length (0 = no constraint).
    /// @param max_len   Maximum word length (0 = no constraint).
    /// @param lang      Language tag.
    /// @return          Array of (word, confidence) pairs matching the
    ///                  Cartesian product of @prefixes × @suffixes.
    #[zbus(out_args("result"))]
    async fn query(
        &self,
        prefixes: Vec<String>,
        suffixes: Vec<String>,
        min_len: u32,
        max_len: u32,
        lang: &str,
    ) -> Vec<(String, f64)> {
        crate::veprintln!(
            "[dbus-server] Query({:?}, {:?}, {}, {}, {})",
            prefixes,
            suffixes,
            min_len,
            max_len,
            lang
        );
        let prefix_opts: Vec<Option<String>> = if prefixes.is_empty() {
            vec![None]
        } else {
            prefixes.into_iter().map(Some).collect()
        };
        let suffix_opts: Vec<Option<String>> = if suffixes.is_empty() {
            vec![None]
        } else {
            suffixes.into_iter().map(Some).collect()
        };
        let queries: Vec<DictionaryQuery> = prefix_opts
            .into_iter()
            .flat_map(|p| {
                suffix_opts.iter().map(move |s| DictionaryQuery {
                    prefix: p.clone(),
                    suffix: s.clone(),
                    min_length: if min_len == 0 {
                        None
                    } else {
                        Some(min_len as usize)
                    },
                    max_length: if max_len == 0 {
                        None
                    } else {
                        Some(max_len as usize)
                    },
                })
            })
            .collect();
        self.handler
            .query(&queries, lang)
            .into_iter()
            .map(|r| (r.word, r.confidence))
            .collect()
    }

    /// Predict the next word(s) given a sequence of context words.
    ///
    /// @param context  Preceding words (space-split is the caller's
    ///                 responsibility).
    /// @param max      Maximum number of predictions to return.
    /// @param lang     Language tag.
    /// @return         Array of (word, confidence) predictions.
    #[zbus(out_args("result"))]
    async fn predict(&self, context: Vec<String>, max: u32, lang: &str) -> Vec<(String, f64)> {
        crate::veprintln!("[dbus-server] Predict({:?}, {}, {})", context, max, lang);
        let ctx: Vec<&str> = context.iter().map(|s| s.as_str()).collect();
        self.handler
            .predict(&ctx, max as usize, lang)
            .into_iter()
            .map(|p| (p.word, p.confidence))
            .collect()
    }

    /// Return the frequency / rank of a word in the dictionary for the
    /// given language.
    ///
    /// @param word  Word to look up.
    /// @param lang  Language tag.
    /// @return      Frequency score (0.0 = unknown word).
    #[zbus(out_args("result"))]
    async fn frequency(&self, word: &str, lang: &str) -> f64 {
        crate::veprintln!("[dbus-server] Frequency({}, {})", word, lang);
        self.handler.frequency(word, lang)
    }
}

/// Register on the session bus and serve forever.
pub async fn run(handler: DaemonHandler) -> zbus::Result<()> {
    let dbus_obj = VerbisageDbus { handler };
    crate::veprintln!("[daemon] connecting to dbus...");
    let _conn = zbus::connection::Builder::session()?
        .name("org.verbisage.Dictionary")?
        .serve_at("/org/verbisage/Dictionary", dbus_obj)?
        .build()
        .await?;
    eprintln!("[daemon] connected to dbus");
    std::future::pending::<()>().await;
    Ok(())
}
