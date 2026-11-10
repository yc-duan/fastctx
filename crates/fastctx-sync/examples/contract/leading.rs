//! Leading host prefixes are real turns without a fabricated opening item.
use fastctx_types::sync::{
    catalog::RecordRef,
    conversation_identity::{NativeId, NativeTurnKey, TurnKey},
    conversation_timeline::{Evidence, TimelineEvent, TimelineFact, TurnDeclaration},
    cursor::Sequence,
    identity::{ObjectId, OperationId, ProducerEpoch, ScopeKey, StreamId},
};

pub fn sample(scope: &ScopeKey) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let conversation = ObjectId::from_bytes([57; 32]);
    let mut turns = Vec::new();
    for (ordinal, native) in [
        NativeTurnKey::ClaudeLeading { session: NativeId::new("retained-session")? },
        NativeTurnKey::CodexLeading { thread: NativeId::new("retained-thread")? },
    ]
    .into_iter()
    .enumerate()
    {
        let turn = TurnDeclaration {
            key: TurnKey { conversation: conversation.clone(), native },
            order: Sequence(ordinal as u64 + 1),
        };
        let evidence = Evidence {
            record: RecordRef {
                stream: StreamId::from_bytes([58; 32]),
                epoch: ProducerEpoch::from_bytes([59; 32]),
                sequence: Sequence(ordinal as u64 + 1),
            },
            operation: OperationId::from_bytes([60 + ordinal as u8; 32]),
            accepted: Sequence(ordinal as u64 + 1),
        };
        let event = TimelineEvent { evidence, fact: TimelineFact::Turn { turn: turn.clone() } };
        assert!(fastctx_sync::conversation::source::event::validate(&conversation, scope, &event)?.is_empty());
        assert!(
            fastctx_sync::conversation::source::event::validate(&ObjectId::from_bytes([61; 32]), scope, &event)
                .is_err()
        );
        let mut session_slot = event.clone();
        if let TimelineFact::Turn { turn } = &mut session_slot.fact {
            turn.order = Sequence(0);
        }
        assert!(fastctx_sync::conversation::source::event::validate(&conversation, scope, &session_slot).is_err());
        turns.push(serde_json::json!({"identity": turn.key.id(), "event": event}));
    }
    Ok(serde_json::json!({"source_gate": "nonzero_turn_without_opening", "turns": turns}))
}
