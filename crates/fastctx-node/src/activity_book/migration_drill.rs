//! The book opens after isolated source damage and leaves no replayable old rows.
use super::{legacy::Context, migrate};
use fastctx_storage::Database;
use fastctx_types::layout::WorldDir;
use std::path::Path;

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let source = Database::open_initialized(&world.path().join("node.sqlite3"), super::SCHEMA, |_| Ok(()))?;
    let target = Database::open_initialized(
        &world.path().join("book.sqlite3"),
        &format!("{}{}", super::SCHEMA, super::SEALS),
        |_| Ok(()),
    )?;
    let service = crate::record_database::register(&world, &source)?;
    service.start()?;
    source.write_now(4096, |db| {
        for key in ["good", "bad"] {
            db.execute(
                "INSERT INTO book_deeds(deed,job,began,script,outcome) VALUES(?1,'job',1,'echo test','done')",
                [key],
            )?;
        }
        for index in 0..40 {
            db.execute(
                "INSERT INTO book_touches(deed,path,how) VALUES('good',?1,'changed')",
                [format!("file-{index:03}")],
            )?;
        }
        db.execute("INSERT INTO book_touches(deed,path,how) VALUES('bad','file','unrecognized')", [])?;
        db.execute("INSERT INTO book_touches(deed,path,how) VALUES('orphan','file','changed')", [])?;
        db.execute("INSERT INTO book_hour(at,runs,failed,busy,files) VALUES(1,1,0,100,40)", [])?;
        db.execute("INSERT INTO book_hour(at,runs,failed,busy,files) VALUES(2,'bad',0,100,1)", [])?;
        Ok(())
    })?;
    let context = Context { source: source.clone(), target: target.clone(), world: world.path().to_path_buf() };
    crate::record_migration::scheduler::request(world.path(), "@book", "book-all", move || {
        migrate::import_all(&context)
    })?;
    assert!(migrate::ready(&target)?);
    let counts = target.read_now(|db| {
        Ok((
            db.query_row("SELECT count(*) FROM book_deeds", [], |row| row.get::<_, i64>(0))?,
            db.query_row("SELECT count(*) FROM book_touches", [], |row| row.get::<_, i64>(0))?,
            db.query_row("SELECT count(*) FROM book_hour", [], |row| row.get::<_, i64>(0))?,
        ))
    })?;
    assert_eq!(counts, (1, 40, 1));
    source.read_now(|db| {
        for table in ["book_deeds", "book_touches", "book_hour"] {
            let count: i64 = db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))?;
            assert_eq!(count, 0);
        }
        Ok(())
    })?;
    assert_eq!(crate::record_migration::archive::damaged_count(world.path())?, 3);
    println!(
        "book migration: 40 touches and one hour verified; invalid deed, orphan touches and invalid hour isolated; old database rows drained"
    );
    sealed(&target)?;
    Ok(())
}

/// A conversation deleted here leaves nothing of it in the book, and takes
/// nothing again; another of the same id from another machine keeps its own.
fn sealed(book: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let deed = |owner: &str, job: usize| format!("{owner} j-{job:04} {}", 1000 + job);
    let desk = format!("name:{}", fastctx_types::MachineName::try_from("DESK")?.key());
    book.write_now(1 << 20, move |db| {
        for (owner, session) in
            [("codex-5ea1ed00 on desk", "codex-5ea1ed00@desk"), ("codex-5ea1ed00 on gpu", "codex-5ea1ed00@gpu")]
        {
            for job in 0..300 {
                let key = deed(owner, job);
                db.execute(
                    "INSERT INTO book_deeds(deed,job,began,session,script,outcome) VALUES(?1,'j',1,?2,'echo','done')",
                    (&key, session),
                )?;
                db.execute("INSERT INTO book_touches(deed,path,how) VALUES(?1,'file','changed')", [&key])?;
                db.execute(
                    "INSERT INTO book_steps(deed,identity,looked,copied,finished) VALUES(?1,'s',1,0,1)",
                    [&key],
                )?;
            }
            db.execute("INSERT INTO book_retired(deed,at) VALUES(?1,1)", [deed(owner, 999)])?;
            for at in 0..200 {
                db.execute(
                    "INSERT INTO book_hour(at,session,runs,failed,busy,files) VALUES(?1,?2,1,0,1,1)",
                    (at, session),
                )?;
            }
        }
        db.execute("INSERT INTO book_sealed(resident,place) VALUES('codex-5ea1ed00',?1)", [&desk])?;
        Ok(())
    })?;
    super::sealing::purge(book)?;
    let left = |owner: &'static str, session: &'static str| {
        book.read_now(move |db| {
            let mut counts = Vec::new();
            for table in ["book_deeds", "book_touches", "book_steps", "book_retired"] {
                counts.push(db.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE deed LIKE ?1"),
                    [format!("{owner} %")],
                    |row| row.get::<_, i64>(0),
                )?);
            }
            counts.push(db.query_row("SELECT count(*) FROM book_hour WHERE session=?1", [session], |row| row.get(0))?);
            Ok(counts)
        })
    };
    assert_eq!(left("codex-5ea1ed00 on desk", "codex-5ea1ed00@desk")?, vec![0, 0, 0, 0, 0]);
    assert_eq!(left("codex-5ea1ed00 on gpu", "codex-5ea1ed00@gpu")?, vec![300, 300, 300, 1, 200]);
    let late = deed("codex-5ea1ed00 on desk", 7);
    assert!(book.read_now(move |db| super::frozen(db, &late))?);
    assert!(!book.read_now(move |db| super::frozen(db, &deed("codex-5ea1ed00 on gpu", 7)))?);
    println!(
        "book sealing: a deleted conversation's 300 deeds, their touches, steps, folded keys and 200 hours gone under another spelling of its machine's name; a late telling refused; another machine's conversation of the same id kept"
    );
    Ok(())
}
