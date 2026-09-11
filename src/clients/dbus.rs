use zbus::blocking::Connection;

/// Synchronous D-Bus client for the `org.verbisage.Dictionary` service.
///
/// Connects to the session bus and sends typed method calls to the
/// `org.verbisage.Dictionary1` interface at `/org/verbisage/Dictionary`.
///
/// # Example
///
/// ```no_run
/// use verbisage::clients::DbusClient;
///
/// let client = DbusClient::new().unwrap();
/// assert!(client.is_correct("hello", "en_US").unwrap());
/// ```
pub struct DbusClient {
    conn: Connection,
    dest: Option<String>,
}

impl DbusClient {
    /// Connect to the session bus and bind to `org.verbisage.Dictionary`.
    pub fn new() -> Result<Self, zbus::Error> {
        crate::veprintln!("[dbus] connecting to session bus ...");
        let conn = Connection::session()?;
        crate::veprintln!("[dbus] connected");
        Ok(Self {
            conn,
            dest: Some("org.verbisage.Dictionary".into()),
        })
    }

    /// Wrap an existing connection (used with P2P in tests).
    pub fn with_connection(conn: Connection) -> Self {
        Self { conn, dest: None }
    }

    fn dest(&self) -> Option<&str> {
        self.dest.as_deref()
    }

    fn call<B: serde::Serialize + zbus::zvariant::Type>(
        &self,
        method: &str,
        dest: Option<&str>,
        body: &B,
    ) -> zbus::Result<zbus::Message> {
        crate::veprintln!("[dbus] -> {} dest={:?}", method, dest);
        let r = self.conn.call_method(
            dest,
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            method,
            body,
        );
        if r.is_ok() {
            crate::veprintln!("[dbus] <- {} ok", method);
        }
        r
    }

    /// Check whether `word` is correctly spelled for the given `lang`.
    pub fn is_correct(&self, word: &str, lang: &str) -> zbus::Result<bool> {
        let msg = self.call("IsCorrect", self.dest(), &(word, lang))?;
        msg.body().deserialize()
    }

    /// Request spelling suggestions for `word` in the given `lang`.
    pub fn suggest(&self, word: &str, max: u32, lang: &str) -> zbus::Result<Vec<String>> {
        let msg = self.call("Suggest", self.dest(), &(word, max, lang))?;
        msg.body().deserialize()
    }

    /// Rank current-word completions and corrections in one bounded response.
    pub fn complete(&self, word: &str, max: u32, lang: &str) -> zbus::Result<Vec<(String, f64)>> {
        let message = self.call("Complete", self.dest(), &(word, max, lang))?;
        message.body().deserialize()
    }

    /// Query the dictionary with prefix/suffix/length constraints for `lang`.
    /// Multiple prefixes and suffixes produce the Cartesian product of queries.
    pub fn query(
        &self,
        prefixes: &[String],
        suffixes: &[String],
        min_len: u32,
        max_len: u32,
        lang: &str,
    ) -> zbus::Result<Vec<(String, f64)>> {
        let msg = self.call(
            "Query",
            self.dest(),
            &(prefixes, suffixes, min_len, max_len, lang),
        )?;
        msg.body().deserialize()
    }

    /// Retrieve a bounded set of completion candidates.
    pub fn query_limited(
        &self,
        prefixes: &[String],
        suffixes: &[String],
        min_len: u32,
        max_len: u32,
        lang: &str,
        max: u32,
    ) -> zbus::Result<Vec<(String, f64)>> {
        let msg = self.call(
            "QueryLimited",
            self.dest(),
            &(prefixes, suffixes, min_len, max_len, lang, max),
        )?;
        msg.body().deserialize()
    }

    /// Predict next word(s) from `context` for the given `lang`.
    pub fn predict(
        &self,
        context: Vec<String>,
        max: u32,
        lang: &str,
    ) -> zbus::Result<Vec<(String, f64)>> {
        let msg = self.call("Predict", self.dest(), &(context, max, lang))?;
        msg.body().deserialize()
    }

    /// Look up the frequency of `word` in `lang`.
    pub fn frequency(&self, word: &str, lang: &str) -> zbus::Result<f64> {
        let msg = self.call("Frequency", self.dest(), &(word, lang))?;
        msg.body().deserialize()
    }

    /// Add `word` to the dictionary for `lang` with the given `frequency`.
    pub fn add_word(
        &self,
        word: &str,
        frequency: f64,
        allow_existing: bool,
        lang: &str,
    ) -> zbus::Result<bool> {
        let msg = self.call(
            "AddWord",
            self.dest(),
            &(word, frequency, allow_existing, lang),
        )?;
        msg.body().deserialize()
    }

    /// Increase the frequency of an n-gram by `delta` for `lang`.
    pub fn bump_ngram(
        &self,
        ngram: Vec<String>,
        delta: f64,
        save_unknown: bool,
        lang: &str,
    ) -> zbus::Result<bool> {
        let msg = self.call(
            "BumpNgram",
            self.dest(),
            &(ngram, delta, save_unknown, lang),
        )?;
        msg.body().deserialize()
    }
}

impl Default for DbusClient {
    fn default() -> Self {
        Self::new().expect("DbusClient::default() requires a session bus")
    }
}

// ---------------------------------------------------------------------------
// P2P integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::Duration;

    use zbus::blocking::connection::Builder;

    use crate::daemon::DaemonHandler;
    use crate::daemon::dbus::VerbisageDbus;
    use crate::dictionary::FileDictionaryBackend;
    use crate::spellcheck::{DictionarySpellChecker, SpellChecker};

    use super::DbusClient;

    fn temp_handler() -> (DaemonHandler, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let dict_path = dir.path().join("words.txt");
        {
            let mut f = std::fs::File::create(&dict_path).unwrap();
            writeln!(f, "hello").unwrap();
            writeln!(f, "world").unwrap();
            writeln!(f, "help").unwrap();
            writeln!(f, "helium").unwrap();
        }
        let file_dict = FileDictionaryBackend::from_multiple_files(&[dict_path], false).unwrap();
        let dict_clone = file_dict.clone();
        let sc: Box<dyn SpellChecker> =
            Box::new(DictionarySpellChecker::new(std::sync::Arc::new(file_dict)));
        let handler = DaemonHandler::new(Box::new(dict_clone), Some(sc), None, "en_US".into());
        (handler, dir)
    }

    #[test]
    fn dbus_client_p2p_basic() {
        let (handler, _dir) = temp_handler();
        let dbus_obj = VerbisageDbus::new(handler);

        let (s1, s2) = UnixStream::pair().unwrap();

        // Server connection on its own thread.
        thread::spawn(move || {
            let _srv = Builder::unix_stream(s1)
                .p2p()
                .server("0123456789abcdef0123456789abcdef")
                .unwrap()
                .serve_at("/org/verbisage/Dictionary", dbus_obj)
                .unwrap()
                .build()
                .unwrap();
            loop {
                thread::sleep(Duration::from_secs(3600));
            }
        });

        thread::sleep(Duration::from_millis(200));

        // Client connection — P2P, no well‑known name.
        let client_conn = Builder::unix_stream(s2).p2p().build().unwrap();
        let client = DbusClient::with_connection(client_conn);

        assert!(client.is_correct("hello", "en_US").unwrap());
        assert!(!client.is_correct("xyzzy", "en_US").unwrap());

        let suggestions = client.suggest("helo", 5, "en_US").unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"hello".to_string()));

        let freq = client.frequency("hello", "en_US").unwrap();
        assert!(freq > 0.0);
        assert_eq!(client.frequency("nonexistent", "en_US").unwrap(), -1.0);

        let hel: &[String] = &[String::from("hel")];
        let results = client.query(hel, &[], 0, 0, "en_US").unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|(w, _)| w == "hello"));

        let hel: &[String] = &[String::from("hel")];
        let o: &[String] = &[String::from("o")];
        let results = client.query(hel, o, 0, 0, "en_US").unwrap();
        assert!(!results.is_empty());

        let results = client.query_limited(hel, &[], 0, 0, "en_US", 1).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].0.starts_with("hel"));
        assert!(
            client
                .query_limited(hel, &[], 0, 0, "en_US", 0)
                .unwrap()
                .is_empty()
        );
        assert!(
            client
                .query_limited(&vec!["h".into(); 17], &[], 0, 0, "en_US", 1)
                .is_err()
        );
        assert!(
            client
                .query_limited(hel, &[], 0, 0, "../../outside", 1)
                .is_err()
        );
        assert!(client.query_limited(hel, &[], 0, 0, "zz_ZZ", 1).is_err());

        let completed = client.complete("helo", 6, "en_US").unwrap();
        assert_eq!(completed[0].0, "hello");
        assert_eq!(client.complete("helo", 1, "en_US").unwrap(), completed[..1]);
        assert!(client.complete("hello", 0, "en_US").unwrap().is_empty());
        assert!(client.complete("", 6, "en_US").unwrap().is_empty());
        assert!(client.complete("two words", 6, "en_US").unwrap().is_empty());
        assert!(client.complete(&"a".repeat(129), 6, "en_US").is_err());
        assert!(client.complete("hello", 6, "../invalid").is_err());
        assert!(client.complete("hello", 6, "zz_ZZ").is_err());

        drop(client);
    }
}
