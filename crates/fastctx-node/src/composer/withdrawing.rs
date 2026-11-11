//! A device takes back a send whose answer it never got. Whether the send
//! arrived is the node's to say: taken back on the device alone, words the
//! node had already handed on would be said twice, or come back to the
//! input and go from it again. Here the send is found by its operation:
//! words still in the queue return to the shared draft, a send that never
//! came is refused when it does, and words that went stay sent.

use super::*;
use fastctx_types::composer::sync::Operation;
use fastctx_types::composer::{ALREADY_SENT, WITHDRAWN};

pub(super) async fn withdraw(node: &Node, id: &ResidentId, state: &mut Stored, send: &str) -> Result<(), Failure> {
    let receipt = receipts::receipt(node, id, state, send).await?;
    let outcome = match receipt {
        Some(_) => None,
        None => receipts::outcome(node, id, state, send).await?,
    };
    take_back(state, send, receipt, outcome)
}

/// What the node knows of the send is its receipt, or, the receipt gone,
/// the outcome kept of it.
fn take_back(
    state: &mut Stored,
    send: &str,
    receipt: Option<Receipt>,
    outcome: Option<Operation>,
) -> Result<(), Failure> {
    // On its way to the host: how that ends is not known yet, and it is not
    // taken out from under the host.
    if state.sending.iter().chain(&state.pending).any(|flight| flight.operation.as_deref() == Some(send)) {
        return Err(Failure::new(ALREADY_SENT));
    }
    let Some(mut receipt) = receipt else {
        return match outcome {
            Some(outcome) if outcome.failure.is_some() => Ok(()),
            Some(_) => Err(Failure::new(ALREADY_SENT)),
            // It never came. Kept as an outcome, the withdrawal refuses it
            // when it does, for as long as a receipt would answer it.
            None => {
                state.outcomes.insert(
                    send.into(),
                    Operation {
                        operation: send.into(),
                        delivery: None,
                        accepted: false,
                        landed: None,
                        failure: Some(WITHDRAWN.into()),
                    },
                );
                Ok(())
            }
        };
    };
    if !matches!(receipt.mutation.change, Change::Send { .. } | Change::SendDocument { .. }) {
        return Err(failure("that operation is not a send"));
    }
    // It did not go, and its words are in the draft already.
    if receipt.failure.is_some() {
        return Ok(());
    }
    // Gone, or released from the queue since, or deleted from it: wherever
    // the words are, they are not here to give back.
    let position = receipt
        .entry
        .as_ref()
        .filter(|_| receipt.landed.is_none())
        .and_then(|entry| state.queue.iter().position(|queued| &queued.id == entry))
        .filter(|position| state.queue[*position].submitted.is_none());
    let Some(position) = position else { return Err(Failure::new(ALREADY_SENT)) };
    if !state.queue[position].editors.is_empty() {
        return Err(failure("the queued message is being edited; it stays in the queue"));
    }
    if state.draft.can_take_back(&state.queue[position].document).is_err() {
        return Err(failure("the input is full; the words stay in the queue, where they can be edited or removed"));
    }
    let entry = state.queue.remove(position);
    state.draft.take_back(entry.document, crate::speech::new_id("back").as_str())?;
    receipt.failure = Some(WITHDRAWN.into());
    state.receipts.insert(send.into(), receipt);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::composer::{Draft, Mutation};

    fn machine() -> MachineName {
        "desktop".parse().expect("a machine name")
    }

    /// A composer with `words` queued by the send `send`, as a send made
    /// while the agent was busy leaves it.
    fn queued(send: &str, words: &str) -> (Stored, Receipt) {
        let mut state = Stored::default();
        let entry = crate::speech::new_id("queued");
        let document = document::Document::from_view(Draft { text: words.into(), ..Draft::default() }).unwrap();
        state.queue.push(Entry {
            id: entry.clone(),
            from: machine(),
            document,
            editors: HashSet::new(),
            error: None,
            submitted: None,
            command: false,
        });
        let receipt = Receipt {
            from: machine(),
            mutation: Mutation { operation: send.into(), change: Change::Send { epoch: 0 } },
            landed: None,
            delivery: None,
            failure: None,
            entry: Some(entry),
        };
        state.receipts.insert(send.into(), receipt.clone());
        (state, receipt)
    }

    #[test]
    fn words_still_queued_return_to_the_draft() {
        let (mut state, receipt) = queued("send-1", "hello");
        take_back(&mut state, "send-1", Some(receipt), None).unwrap();
        assert!(state.queue.is_empty());
        assert_eq!(state.draft.view().text, "hello");
        assert_eq!(state.receipts["send-1"].failure.as_deref(), Some(WITHDRAWN));
        // Asked again, the send did not go and there is nothing more to do.
        let again = state.receipts["send-1"].clone();
        take_back(&mut state, "send-1", Some(again), None).unwrap();
        assert_eq!(state.draft.view().text, "hello");
    }

    #[test]
    fn a_send_that_never_came_is_refused_when_it_does() {
        let mut state = Stored::default();
        take_back(&mut state, "send-2", None, None).unwrap();
        assert_eq!(state.outcomes["send-2"].failure.as_deref(), Some(WITHDRAWN));
    }

    #[test]
    fn words_on_their_way_to_the_host_stay_sent() {
        let (mut state, receipt) = queued("send-3", "hello");
        let entry = receipt.entry.clone().unwrap();
        let mut flight = state.reserve(&entry, false, true).unwrap();
        flight.operation = Some("send-3".into());
        state.sending = Some(flight);
        let refused = take_back(&mut state, "send-3", Some(receipt), None).unwrap_err();
        assert_eq!(refused.message, ALREADY_SENT);
        assert_eq!(state.queue.len(), 1);
    }

    #[test]
    fn words_that_went_out_stay_sent() {
        let (mut state, mut receipt) = queued("send-4", "hello");
        state.queue.clear();
        receipt.landed = Some(Landed::NewTurn { turn: "turn-1".into() });
        let refused = take_back(&mut state, "send-4", Some(receipt), None).unwrap_err();
        assert_eq!(refused.message, ALREADY_SENT);
        assert!(state.draft.view().text.is_empty());
        // Released from the queue by a later turn's end, with no word of it
        // on the send's receipt: they are not in the queue to give back.
        let (mut state, receipt) = queued("send-5", "hello");
        state.queue.clear();
        let refused = take_back(&mut state, "send-5", Some(receipt), None).unwrap_err();
        assert_eq!(refused.message, ALREADY_SENT);
    }

    #[test]
    fn only_a_send_is_withdrawn() {
        let mut state = Stored::default();
        let receipt = Receipt {
            from: machine(),
            mutation: Mutation {
                operation: "move-1".into(),
                change: Change::Delete { id: crate::speech::new_id("queued") },
            },
            landed: None,
            delivery: None,
            failure: None,
            entry: None,
        };
        assert!(take_back(&mut state, "move-1", Some(receipt), None).is_err());
    }
}
