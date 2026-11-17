//! Writer lifecycle checks inject broken guards and inspect durable outcomes.

use fastctx_storage::{
    Database, Error, Guard, Result,
    authorization::{Write, WritePolicy},
    guard::{Action, Change, CompositeGuard, MigrationPurpose},
    rusqlite::{Connection, types::ValueRef},
};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const SCHEMA: &str =
    "CREATE TABLE events(id INTEGER PRIMARY KEY,body TEXT NOT NULL); CREATE TABLE hidden(value INTEGER);";
const INSERT: &str = "INSERT INTO events(id,body) VALUES(?1,?2)";

struct Policy(Arc<AtomicBool>);
impl WritePolicy for Policy {
    fn permits(&self, write: Write<'_>) -> bool {
        write.table != "hidden" && self.0.load(Ordering::Acquire)
    }
}
struct Faults {
    gate: Arc<AtomicBool>,
    mode: String,
}
impl Guard for Faults {
    fn check(&mut self, change: &Change<'_>) -> Result<()> {
        if change.table() == "hidden" || !self.gate.load(Ordering::Acquire) {
            return Err(Error::new("injected closed write gate"));
        }
        if change.table() == "events" && change.action() != Action::Delete {
            let ValueRef::Text(body) = change.after(1)? else {
                return Err(Error::new("expected event body"));
            };
            self.mode = std::str::from_utf8(body).map_err(|error| Error::new(error.to_string()))?.to_owned();
            assert_ne!(self.mode, "hook-panic", "injected hook panic");
        }
        Ok(())
    }
    fn validate_operation(&mut self, db: &Connection) -> Result<()> {
        match self.mode.as_str() {
            "validate-error" => Err(Error::new("injected validation refusal")),
            "validate-panic" => panic!("injected validation panic"),
            "validate-write" => {
                let _ = db.execute("INSERT INTO events(id,body) VALUES(999,'validator')", []);
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn write_policy(&self) -> Option<Arc<dyn WritePolicy>> {
        Some(Arc::new(Policy(self.gate.clone())))
    }
    fn ended(&mut self, _: bool) {
        self.mode.clear();
    }
    fn committed(&mut self, _: bool) {
        self.mode.clear();
    }
}
#[derive(Default)]
struct Observed {
    committed: Vec<i64>,
    refused: Vec<i64>,
}
struct Tracker {
    seen: Arc<Mutex<Observed>>,
    operation: Vec<i64>,
    transaction: Vec<i64>,
}
impl Guard for Tracker {
    fn check(&mut self, change: &Change<'_>) -> Result<()> {
        if change.table() == "events"
            && change.action() != Action::Delete
            && let ValueRef::Integer(id) = change.after(0)?
        {
            self.operation.push(id);
        }
        Ok(())
    }
    fn ended(&mut self, kept: bool) {
        if kept {
            self.transaction.append(&mut self.operation);
        } else {
            self.seen.lock().expect("observer").refused.append(&mut self.operation);
        }
    }
    fn committed(&mut self, stood: bool) {
        if stood {
            self.seen.lock().expect("observer").committed.append(&mut self.transaction);
        } else {
            self.transaction.clear();
        }
        self.operation.clear();
    }
}

pub(super) fn run(root: &Path) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = root.join("guards.sqlite3");
    let gate = Arc::new(AtomicBool::new(true));
    let seen = Arc::new(Mutex::new(Observed::default()));
    let database = Database::open_guarded(
        &path,
        SCHEMA,
        |_| {
            Ok(Box::new(CompositeGuard::new(vec![
                Box::new(Faults { gate: gate.clone(), mode: String::new() }),
                Box::new(Tracker { seen: seen.clone(), operation: Vec::new(), transaction: Vec::new() }),
            ])))
        },
        |_| Ok(()),
    )?;
    database.write_now(32, |db| {
        db.prepare_cached(INSERT)?.execute((1, "kept"))?;
        Ok(())
    })?;
    for (id, body) in [(2, "hook-panic"), (3, "validate-error"), (4, "validate-panic"), (5, "validate-write")] {
        assert!(
            database
                .write_now(32, move |db| {
                    db.prepare_cached(INSERT)?.execute((id, body))?;
                    Ok(())
                })
                .is_err()
        );
    }
    let changing = gate.clone();
    assert!(
        database
            .write_now(32, move |db| {
                let mut statement = db.prepare_cached(INSERT)?;
                changing.store(false, Ordering::Release);
                let result = statement.execute((6, "cached-statement"));
                changing.store(true, Ordering::Release);
                result?;
                Ok(())
            })
            .is_err(),
        "a cached statement bypassed its row guard"
    );
    assert!(
        database
            .write_now(32, |db| {
                db.execute(INSERT, (7, "before forbidden DDL"))?;
                let _ = db.execute_batch("CREATE TABLE bypass(value INTEGER)");
                Ok(())
            })
            .is_err(),
        "ignored authorization failure committed partial work"
    );
    assert!(
        database
            .write_now(0, |db| {
                db.execute("INSERT INTO hidden(value) VALUES(1)", [])?;
                Ok(())
            })
            .is_err()
    );
    assert!(
        database
            .write_now(0, |db| {
                db.execute_batch("ATTACH ':memory:' AS bypass")?;
                Ok(())
            })
            .is_err()
    );
    assert!(
        database
            .write_now(32, |db| {
                let _transfer = fastctx_storage::guard::enter_migration(MigrationPurpose::ConversationTransfer);
                db.execute(INSERT, (10, "hook-panic"))?;
                Ok(())
            })
            .is_err(),
        "a migration purpose swallowed a guard panic"
    );
    for (index, sql) in
        ["COMMIT", "ROLLBACK", "RELEASE _rusqlite_sp", "ROLLBACK TO _rusqlite_sp"].into_iter().enumerate()
    {
        assert!(
            database
                .write_now(32, move |db| {
                    db.execute(INSERT, (20 + index as i64, "transaction escape"))?;
                    let _ = db.prepare_cached(sql).and_then(|mut statement| statement.execute([]));
                    Ok(())
                })
                .is_err(),
            "a callback escaped its transaction with {sql}"
        );
    }
    database.write_now(32, |db| {
        db.execute_batch("SAVEPOINT legacy_record_batch; RELEASE legacy_record_batch")?;
        db.execute(INSERT, (9, "kept after failures"))?;
        Ok(())
    })?;
    let ids = database.read_now(|db| {
        Ok(db
            .prepare("SELECT id FROM events ORDER BY id")?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    assert_eq!(ids, [1, 9]);
    assert!(!database.read_now(|db| Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='bypass')",
        [],
        |row| row.get::<_, bool>(0)
    )?))?);
    let observed = seen.lock().expect("observer");
    assert_eq!(observed.committed, [1, 9]);
    for id in [2, 3, 4, 5, 6, 7, 10, 20, 21, 22, 23] {
        assert!(observed.refused.contains(&id), "another component missed the lifecycle of {id}");
    }
    println!(
        "guard lifecycle: hook and validator failures rolled back; cached writes, undeclared DDL, ignored authorization errors, transaction escape and transfer panic refused; later writes committed"
    );
    Ok(())
}
