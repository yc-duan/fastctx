use super::*;

#[test]
fn a_closed_slot_requires_its_exact_recovery_anchor() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(SCHEMA).unwrap();
    let (first, first_head) = append(&db, "checkpoint", &[1; 32]).unwrap();
    let (second, _) = append(&db, "checkpoint", &[2; 32]).unwrap();
    let (anchor, anchor_head) = append(&db, "branch-closed", &[3; 32]).unwrap();
    db.execute(
        "INSERT INTO closed_head_slots VALUES (?1, ?2, ?3)",
        params![first as i64, anchor as i64, &anchor_head[..]],
    )
    .unwrap();
    db.execute("INSERT INTO closed_heads VALUES (?1, ?2)", params![first as i64, &first_head[..]]).unwrap();

    assert!(closed_head(&db, first, &[4; 32]).unwrap());
    assert!(!closed_head(&db, second, &[4; 32]).unwrap());
    db.execute("UPDATE hub_chain SET head = ?1 WHERE seq = ?2", params![&[5_u8; 32][..], anchor as i64]).unwrap();
    assert!(!closed_head(&db, first, &[4; 32]).unwrap());
    db.execute("DELETE FROM hub_chain WHERE seq = ?1", [anchor as i64]).unwrap();
    assert!(!closed_head(&db, first, &[4; 32]).unwrap());
    assert!(closed_head(&db, first, &first_head).unwrap());
}
