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
/// assert!(client.is_correct("hello").unwrap());
/// ```
pub struct DbusClient {
    conn: Connection,
    dest: Option<String>,
}

impl DbusClient {
    /// Connect to the session bus and bind to `org.verbisage.Dictionary`.
    pub fn new() -> Result<Self, zbus::Error> {
        let conn = Connection::session()?;
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

    pub fn is_correct(&self, word: &str) -> zbus::Result<bool> {
        let msg = self.conn.call_method(
            self.dest(),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "IsCorrect",
            &(word,),
        )?;
        msg.body().deserialize()
    }

    pub fn suggest(&self, word: &str, max: u32) -> zbus::Result<Vec<String>> {
        let msg = self.conn.call_method(
            self.dest(),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "Suggest",
            &(word, max),
        )?;
        msg.body().deserialize()
    }

    pub fn query(
        &self,
        prefix: &str,
        suffix: &str,
        min_len: u32,
        max_len: u32,
    ) -> zbus::Result<Vec<(String, f64)>> {
        let msg = self.conn.call_method(
            self.dest(),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "Query",
            &(prefix, suffix, min_len, max_len),
        )?;
        msg.body().deserialize()
    }

    pub fn predict(&self, context: Vec<String>, max: u32) -> zbus::Result<Vec<(String, f64)>> {
        let msg = self.conn.call_method(
            self.dest(),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "Predict",
            &(context, max),
        )?;
        msg.body().deserialize()
    }

    pub fn frequency(&self, word: &str) -> zbus::Result<f64> {
        let msg = self.conn.call_method(
            self.dest(),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "Frequency",
            &(word,),
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
        let file_dict = FileDictionaryBackend::from_multiple_files(&[dict_path]).unwrap();
        let dict_clone = file_dict.clone();
        let sc: Box<dyn SpellChecker> =
            Box::new(DictionarySpellChecker::new(std::sync::Arc::new(file_dict)));
        let handler = DaemonHandler::new(Box::new(dict_clone), Some(sc), None);
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

        assert!(client.is_correct("hello").unwrap());
        assert!(!client.is_correct("xyzzy").unwrap());

        let suggestions = client.suggest("helo", 5).unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.contains(&"hello".to_string()));

        let freq = client.frequency("hello").unwrap();
        assert!(freq > 0.0);
        assert_eq!(client.frequency("nonexistent").unwrap(), 0.0);

        let results = client.query("hel", "", 0, 0).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|(w, _)| w == "hello"));

        let results = client.query("hel", "o", 0, 0).unwrap();
        assert!(!results.is_empty());

        drop(client);
    }
}
