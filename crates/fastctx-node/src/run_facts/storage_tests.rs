//! Tests of `storage`: what changed of a conversation's runs' facts, call
//! by call.
use super::*;

/// What changed after a version is known call by call from when calls
/// were first noted, and only while no change went unnoted between.
#[test]
fn the_calls_changed_since_a_version_are_known_while_every_change_named_its_call() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(sql::SCHEMA).unwrap();
    let conversation = "conversation";
    let calls = |after, most| changed_in(&db, conversation, after, most).unwrap();
    let named = |names: &[&str], through| Some((names.iter().map(|name| name.to_string()).collect::<Vec<_>>(), through));
    assert_eq!(calls(0, 10), (0, named(&[], 0)));
    sql::changed(&db, conversation, "a").unwrap();
    sql::changed(&db, conversation, "b").unwrap();
    sql::changed(&db, conversation, "a").unwrap();
    assert_eq!(calls(0, 10), (3, named(&["b", "a"], 3)));
    assert_eq!(calls(2, 10), (3, named(&["a"], 3)));
    assert_eq!(calls(3, 10), (3, named(&[], 3)));
    // A page at a time: the first, then from where it ended.
    assert_eq!(calls(0, 1), (3, named(&["b"], 2)));
    assert_eq!(calls(2, 1), (3, named(&["a"], 3)));
    // A change an older node made, naming no call: what came after it
    // is known from the next change on, nothing before.
    db.execute("UPDATE run_versions SET version=version+1 WHERE conversation=?1", [conversation]).unwrap();
    assert_eq!(calls(3, 10), (4, None));
    sql::changed(&db, conversation, "c").unwrap();
    assert_eq!(calls(3, 10), (5, None));
    assert_eq!(calls(4, 10), (5, named(&["c"], 5)));
    // Changes made before any was noted call by call are not known.
    let other = "other";
    db.execute("INSERT INTO run_versions(conversation,version) VALUES(?1,7)", [other]).unwrap();
    sql::changed(&db, other, "d").unwrap();
    assert_eq!(changed_in(&db, other, 6, 10).unwrap(), (8, None));
    assert_eq!(changed_in(&db, other, 7, 10).unwrap(), (8, named(&["d"], 8)));
    // Another conversation's are its own.
    assert_eq!(calls(4, 10), (5, named(&["c"], 5)));
}
