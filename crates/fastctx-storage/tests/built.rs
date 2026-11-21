//! How the bundled SQLite is built (`.cargo/config.toml`).

use fastctx_storage::rusqlite::{Connection, ffi};

/// SQLite counts no memory: with the count on, every allocation and release
/// on every thread takes one lock of the whole process.
#[test]
fn sqlite_counts_no_memory() {
    let connection = Connection::open_in_memory().expect("open");
    connection.execute_batch("CREATE TABLE kept(body BLOB); INSERT INTO kept VALUES(zeroblob(1 << 20));").expect("write");
    let built: bool =
        connection.query_row("SELECT sqlite_compileoption_used('DEFAULT_MEMSTATUS=0')", [], |row| row.get(0)).expect("ask");
    assert!(built, "the bundled SQLite was built without SQLITE_DEFAULT_MEMSTATUS=0");
    // SAFETY: reads a counter of the library; no pointer is passed.
    let used = unsafe { ffi::sqlite3_memory_used() };
    assert_eq!(used, 0, "SQLite counts the memory it uses");
}
