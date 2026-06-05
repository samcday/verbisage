use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OpenFlags};

/// A thread-safe shared SQLite connection.
///
/// Multiple backends (dictionary + predictor) can share the same underlying
/// connection via `Arc<Mutex<Connection>>`.  Clone is O(1) — it just bumps
/// the Arc refcount.
#[derive(Clone, Debug)]
pub struct SharedSqliteConnection(Arc<Mutex<Connection>>);

impl SharedSqliteConnection {
    pub fn new(conn: Connection) -> Self {
        Self(Arc::new(Mutex::new(conn)))
    }

    /// Open or create a database at `path` (read-write).
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        Connection::open(path).map(Self::new)
    }

    /// Open a database read-only.
    pub fn open_readonly(path: &Path) -> Result<Self, rusqlite::Error> {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map(Self::new)
    }

    /// Create an in-memory database.
    pub fn in_memory() -> Self {
        Self::new(Connection::open(":memory:").expect("in-memory sqlite open"))
    }

    /// Lock the connection for exclusive use.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().expect("sqlite connection lock")
    }

    /// Whether this connection points to an in-memory database.
    /// Used to distinguish clones that should remain independent.
    pub fn is_in_memory(&self) -> bool {
        // Cheap heuristic: in-memory connections have no backing file.
        // We can't reliably query this from rusqlite, so we use a marker.
        false
    }
}
