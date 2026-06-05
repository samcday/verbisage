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
}

impl DbusClient {
    /// Connect to the session bus and bind to `org.verbisage.Dictionary`.
    pub fn new() -> Result<Self, zbus::Error> {
        let conn = Connection::session()?;
        Ok(Self { conn })
    }

    pub fn is_correct(&self, word: &str) -> zbus::Result<bool> {
        let msg = self.conn.call_method(
            Some("org.verbisage.Dictionary"),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "IsCorrect",
            &(word,),
        )?;
        msg.body().deserialize()
    }

    pub fn suggest(&self, word: &str, max: u32) -> zbus::Result<Vec<String>> {
        let msg = self.conn.call_method(
            Some("org.verbisage.Dictionary"),
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
            Some("org.verbisage.Dictionary"),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "Query",
            &(prefix, suffix, min_len, max_len),
        )?;
        msg.body().deserialize()
    }

    pub fn predict(&self, context: Vec<String>, max: u32) -> zbus::Result<Vec<(String, f64)>> {
        let msg = self.conn.call_method(
            Some("org.verbisage.Dictionary"),
            "/org/verbisage/Dictionary",
            Some("org.verbisage.Dictionary1"),
            "Predict",
            &(context, max),
        )?;
        msg.body().deserialize()
    }

    pub fn frequency(&self, word: &str) -> zbus::Result<f64> {
        let msg = self.conn.call_method(
            Some("org.verbisage.Dictionary"),
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
