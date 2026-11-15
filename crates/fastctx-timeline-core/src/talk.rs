pub mod merging;

/// How soon after its turn began words count as the ones that opened it.
pub const OPENING_MS: i64 = 2_000;

/// The turn an event belongs to, where it says.
pub fn turn_of(happened: &fastctx_types::session::Happened) -> Option<&str> {
    use fastctx_types::session::Happened;
    match happened {
        Happened::TurnStarted { turn }
        | Happened::TurnEnded { turn, .. }
        | Happened::Began { turn, .. }
        | Happened::Updated { turn, .. }
        | Happened::Grew { turn, .. }
        | Happened::Finished { turn, .. }
        | Happened::Steps { turn, .. } => (!turn.is_empty()).then_some(turn),
        Happened::Heard { turn, .. } | Happened::Health { turn, .. } | Happened::Changed { turn, .. } => {
            turn.as_deref()
        }
        Happened::Context { .. }
        | Happened::Titled { .. }
        | Happened::DeliveryUncertain { .. }
        | Happened::Realtime { .. }
        | Happened::RealtimeGrew { .. } => None,
    }
}
