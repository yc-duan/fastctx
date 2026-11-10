//! Opt-in live and unpaired streams without sockets.
use super::*;

pub(crate) fn seed(hub: &Hub, standings: &[Standing]) -> Vec<oneshot::Receiver<String>> {
    let mut receivers = Vec::new();
    for (i, standing) in standings.iter().enumerate() {
        register(
            hub,
            &Ticket { stream: i as u64, source: standing.from.clone(), sink: standing.to.clone() },
            standing,
            &standing.from,
            0,
        );
        let (cut, receiver) = oneshot::channel();
        hub.data
            .lock()
            .expect("streams")
            .live
            .insert(1000 + i as u64, Live { serial: i as u64, standings: vec![standing.clone()], cut: Some(cut) });
        receivers.push(receiver);
        let (hand_over, _connection) = oneshot::channel();
        let (called_off, waiting) = oneshot::channel();
        hub.data.lock().expect("streams").waiting.insert(
            i as u64,
            WaitingEnd {
                waiting: Waiting { serial: i as u64, end: End::Source, member: standing.from.clone() },
                hand_over,
                called_off,
            },
        );
        receivers.push(waiting);
    }
    receivers
}

pub(crate) fn snapshot(hub: &Hub) -> (Vec<u64>, Vec<(u64, bool)>) {
    let pairing = hub.data.lock().expect("streams");
    let mut registered: Vec<_> = pairing.registered.keys().copied().collect();
    let mut live: Vec<_> = pairing.live.iter().map(|(key, value)| (*key, value.cut.is_some())).collect();
    registered.sort_unstable();
    live.sort_unstable();
    (registered, live)
}
