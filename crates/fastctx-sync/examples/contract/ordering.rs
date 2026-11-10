//! Session and sibling-placement wire shapes; positions remain opaque anchors.
use fastctx_types::sync::{
    conversation_timeline::{DisplayKind, DisplayOrder, DisplaySlot, Placement, PlacementProof},
    cursor::Sequence,
    identity::ObjectId,
    timeline::{TimelineChange, TimelineItem},
};

pub fn sample(item: &TimelineItem) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let session = Placement {
        turn: None,
        slot: DisplaySlot::Session,
        proof: PlacementProof::Causal,
        order: Some(DisplayOrder::Native { sequence: Sequence(1) }),
    };
    let gap = Placement {
        turn: item.turn.clone(),
        slot: DisplaySlot::Sibling,
        proof: PlacementProof::Causal,
        order: Some(DisplayOrder::Gap {
            before: Some(item.id.clone()),
            at: "2026-01-01T00:00:20Z".parse()?,
            sequence: Sequence(7),
            identity: ObjectId::from_bytes([48; 32]),
        }),
    };
    let mut title = item.clone();
    title.turn = None;
    title.position.turn = Sequence(0);
    title.display.kind = DisplayKind::System;
    title.validate_channel()?;
    Ok(serde_json::json!({"session": session, "gap": gap, "session_item": TimelineChange::Put { item: title }}))
}
