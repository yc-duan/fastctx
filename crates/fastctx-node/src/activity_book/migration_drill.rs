//! The book opens after isolated source damage and leaves no replayable old rows.
use super::{legacy::Context, migrate};
use fastctx_storage::Database;
use fastctx_types::layout::WorldDir;
use std::path::Path;

pub(crate) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let world = WorldDir::under(root);
    let source = Database::open_initialized(&world.path().join("node.sqlite3"), super::SCHEMA, |_| Ok(()))?;
    let target = Database::open_initialized(&world.path().join("book.sqlite3"), super::SCHEMA, |_| Ok(()))?;
    let service = crate::record_database::register(&world, &source);
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
    Ok(())
}
