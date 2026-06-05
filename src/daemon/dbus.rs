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

#[interface(name = "org.verbisage.Dictionary1")]
impl VerbisageDbus {
    async fn is_correct(&self, word: &str, lang: &str) -> bool {
        self.handler.is_correct(word, lang)
    }

    async fn suggest(&self, word: &str, max: u32, lang: &str) -> Vec<String> {
        self.handler.suggest(word, max as usize, lang)
    }

    async fn query(
        &self,
        prefix: &str,
        suffix: &str,
        min_len: u32,
        max_len: u32,
        lang: &str,
    ) -> Vec<(String, f64)> {
        let query = DictionaryQuery {
            prefix: if prefix.is_empty() {
                None
            } else {
                Some(prefix.to_string())
            },
            suffix: if suffix.is_empty() {
                None
            } else {
                Some(suffix.to_string())
            },
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
        };
        self.handler
            .query(&query, lang)
            .into_iter()
            .map(|r| (r.word, r.confidence))
            .collect()
    }

    async fn predict(&self, context: Vec<String>, max: u32, lang: &str) -> Vec<(String, f64)> {
        let ctx: Vec<&str> = context.iter().map(|s| s.as_str()).collect();
        self.handler
            .predict(&ctx, max as usize, lang)
            .into_iter()
            .map(|p| (p.word, p.confidence))
            .collect()
    }

    async fn frequency(&self, word: &str, lang: &str) -> f64 {
        self.handler.frequency(word, lang)
    }
}

/// Register on the session bus and serve forever.
pub async fn run(handler: DaemonHandler) -> zbus::Result<()> {
    let dbus_obj = VerbisageDbus { handler };
    let _conn = zbus::connection::Builder::session()?
        .name("org.verbisage.Dictionary")?
        .serve_at("/org/verbisage/Dictionary", dbus_obj)?
        .build()
        .await?;
    std::future::pending::<()>().await;
    Ok(())
}
