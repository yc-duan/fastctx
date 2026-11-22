use super::{LAST_ACTION, LAST_ACTION_WITHIN};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, StatementStatus, params};

fn arrival(at: i128) -> [u8; 16] {
    (at ^ i128::MIN).to_be_bytes()
}

#[test]
fn the_last_action_is_one_row_read_without_sorting() {
    let db = Connection::open_in_memory().expect("open");
    db.execute_batch(crate::records::SCHEMA).expect("schema");
    for (position, family, at) in [(1, "actions", 10), (2, "messages", 20), (3, "actions", 30), (4, "actions", 40), (5, "seen", 50)] {
        db.execute(
            "INSERT INTO conversation_records(position,conversation,family,identity,body,modified,arrival) VALUES(?1,'c',?2,?3,?4,'',?5)",
            params![position, family, format!("row-{position}"), format!("{{\"n\":{position}}}").into_bytes(), arrival(at).as_slice()],
        )
        .expect("insert");
    }
    let last: Option<Vec<u8>> = db.query_row(LAST_ACTION, ["c"], |row| row.get(0)).optional().expect("last");
    assert_eq!(last.as_deref(), Some(br#"{"n":4}"#.as_slice()));
    let within = |from: i128, until: i128| -> Option<Vec<u8>> {
        db.query_row(LAST_ACTION_WITHIN, params!["c", arrival(from).as_slice(), arrival(until).as_slice()], |row| row.get(0))
            .optional()
            .expect("within")
    };
    assert_eq!(within(0, 40).as_deref(), Some(br#"{"n":3}"#.as_slice()));
    assert_eq!(within(0, 10), None);
    assert_eq!(within(31, 1000).as_deref(), Some(br#"{"n":4}"#.as_slice()));
    let none: Option<Vec<u8>> = db.query_row(LAST_ACTION, ["other"], |row| row.get(0)).optional().expect("none");
    assert_eq!(none, None);
    // Written later than one that happened after it, and two arrived at
    // one moment: the last written of the span is told, not the last to
    // arrive nor the greater identity.
    for (position, identity, at) in [(6, "b", 35), (7, "a", 35), (8, "z", 15)] {
        db.execute(
            "INSERT INTO conversation_records(position,conversation,family,identity,body,modified,arrival) VALUES(?1,'c','actions',?2,?3,'',?4)",
            params![position, identity, format!("{{\"n\":{position}}}").into_bytes(), arrival(at).as_slice()],
        )
        .expect("insert");
    }
    assert_eq!(within(0, 40).as_deref(), Some(br#"{"n":8}"#.as_slice()));
    assert_eq!(within(31, 40).as_deref(), Some(br#"{"n":7}"#.as_slice()));
    // What a conversation wrote after a span costs the asking nothing: five
    // thousand actions later, the span's last is found in a few steps.
    for position in 100..5100_i64 {
        db.execute(
            "INSERT INTO conversation_records(position,conversation,family,identity,body,modified,arrival) VALUES(?1,'c','actions',?2,'{}','',?3)",
            params![position, format!("later-{position}"), arrival(1000 + i128::from(position)).as_slice()],
        )
        .expect("insert");
    }
    for (from, until, found) in [(0, 40, Some(br#"{"n":8}"#.as_slice())), (41, 900, None)] {
        let mut asked = db.prepare(LAST_ACTION_WITHIN).expect("prepare");
        let told: Option<Vec<u8>> = asked
            .query_row(params!["c", arrival(from).as_slice(), arrival(until).as_slice()], |row| row.get(0))
            .optional()
            .expect("within");
        assert_eq!(told.as_deref(), found);
        let steps = asked.get_status(StatementStatus::VmStep);
        assert!(steps < 200, "{steps} steps for the span {from}..{until}");
    }
    for (sql, index) in [(LAST_ACTION, "conversation_records_order"), (LAST_ACTION_WITHIN, "COVERING INDEX conversation_records_arrival")] {
        let mut explained = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).expect("explain");
        let bound = vec!["c"; explained.parameter_count()];
        let plan: Vec<String> = explained
            .query_map(fastctx_storage::rusqlite::params_from_iter(bound), |row| row.get::<_, String>(3))
            .expect("plan")
            .collect::<Result<_, _>>()
            .expect("steps");
        assert!(plan.iter().any(|step| step.contains(index)), "{plan:?}");
        assert!(plan.iter().all(|step| !step.contains("TEMP B-TREE")), "{plan:?}");
    }
}

/// A family of few rows or none, in a conversation that wrote much else, is
/// paged through at the cost of its own rows (`records::PAGE`).
#[test]
fn a_page_of_a_family_costs_that_family() {
    let db = Connection::open_in_memory().expect("open");
    db.execute_batch(crate::records::SCHEMA).expect("schema");
    for position in 1..=5000_i64 {
        db.execute(
            "INSERT INTO conversation_records(position,conversation,family,identity,body,modified) VALUES(?1,'c','events',?2,'{}','')",
            params![position, format!("event-{position}")],
        )
        .expect("insert");
    }
    db.execute(
        "INSERT INTO conversation_records(position,conversation,family,identity,body,modified) VALUES(6000,'c','controls','one','{}','')",
        [],
    )
    .expect("insert");
    for (family, rows) in [("controls", 1), ("uncertain-inputs", 0)] {
        let mut page = db.prepare(crate::records::PAGE).expect("prepare");
        let found = page.query_map(params!["c", family, 0, 128], |row| row.get::<_, i64>(0)).expect("page").count();
        assert_eq!(found, rows);
        let steps = page.get_status(StatementStatus::VmStep);
        assert!(steps < 100, "{steps} steps for a page of {family}");
    }
}
