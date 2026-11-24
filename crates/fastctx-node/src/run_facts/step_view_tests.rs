//! Tests of `step_view`: deleting a call's steps.
use super::*;
use fastctx_storage::rusqlite::Connection;

/// Steps are numbered per conversation: deleting one call's steps leaves
/// another conversation's steps at the same positions, and its folds.
#[test]
fn deleting_a_call_s_steps_leaves_other_conversations_steps_at_the_same_positions() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(sql::SCHEMA).unwrap();
    for (conversation, call) in [("a", "j-1"), ("b", "j-1"), ("b", "j-2")] {
        for step in 1..=70 {
            let position = if call == "j-2" { 100 + step } else { step };
            db.execute(
                "INSERT INTO run_steps(position,conversation,call,id,body,fingerprint,finished,machine,kind,first,last)
                VALUES(?1,?2,?3,?4,x'00','f',1,'m','k',0,0)",
                (position, conversation, call, format!("s{step}")),
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO run_folded(conversation,call,machine,kind,count,first,last) VALUES(?1,?2,'m','k',1,0,0)",
            (conversation, call),
        )
        .unwrap();
    }
    for statement in DELETE_BATCH {
        while db.execute(statement, ("a", "j-1")).unwrap() > 0 {}
    }
    let count = |table: &str, conversation: &str, call: &str| -> i64 {
        db.query_row(
            &format!("SELECT count(*) FROM {table} WHERE conversation=?1 AND call=?2"),
            (conversation, call),
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(count("run_steps", "a", "j-1"), 0);
    assert_eq!(count("run_folded", "a", "j-1"), 0);
    assert_eq!(count("run_steps", "b", "j-1"), 70);
    assert_eq!(count("run_steps", "b", "j-2"), 70);
    assert_eq!(count("run_folded", "b", "j-1"), 1);
    assert_eq!(count("run_folded", "b", "j-2"), 1);
}
