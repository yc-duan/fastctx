//! The town's mail: letters other machines' towns sent to residents of this one
//! (`specs/world-network.md`, "寄信").

use super::{Letter, Peer};
use crate::town::core::{Delivery, Town};
use crate::world::mail::InboxItem;
use std::collections::HashSet;
use std::sync::Arc;

impl Town {
    /// Opens this machine's letters as they arrive, in the order the hub delivered them, for as
    /// long as the node runs. A letter is acknowledged once opened, so it is opened once; one
    /// that cannot be read or carried out is kept among the delivery failures with the reason,
    /// not opened again, since a second try would find the same.
    pub(crate) async fn read_mail(self: Arc<Self>) {
        // Letters opened whose acknowledgement failed: still in the inbox, not to open twice.
        let mut opened: HashSet<String> = HashSet::new();
        loop {
            let letters = match self.world().next_mail(&opened).await {
                Ok(letters) => letters,
                Err(error) => {
                    self.log(format!("cannot read this machine's mail: {error}"));
                    self.world().mail_changed().await;
                    continue;
                }
            };
            for item in letters {
                if let Err(reason) = self.open_letter(&item).await {
                    self.log(format!("a letter from {} was not carried out: {reason}", item.from));
                    if let Err(error) = crate::world::delivery::record_unopened(&self.world().paths, &item, &reason) {
                        self.log(format!("cannot keep the letter from {} among the delivery failures: {error}", item.from));
                    }
                }
                if let Err(error) = self.world().acknowledge_mail(&item.receipt) {
                    self.log(format!("cannot mark a letter from {} as read: {error}", item.from));
                    opened.insert(item.receipt.clone());
                }
            }
        }
    }

    /// Carries out one letter. Words another resident could not be given are told back to it;
    /// anything else that fails is the error.
    async fn open_letter(&self, item: &InboxItem) -> Result<(), String> {
        let letter: Letter = serde_json::from_value(item.letter.body.clone()).map_err(|error| format!("it is not a letter a town writes: {error}"))?;
        match letter {
            Letter::Deliver { to, delivery } => {
                // The delivery waits under this machine's own id: an id another machine chose
                // never names a file here.
                let delivery = Delivery { id: crate::town::core::delivery_id()?, ..delivery };
                self.deliver_or_hold(&to, delivery).await.map(|_| ()).map_err(|error| format!("cannot deliver words to {to}: {error}"))
            }
            Letter::Speak { from, to, message } => {
                let speaker = Peer::new(&item.from, &from);
                if let Err(error) = self.speak(&speaker, &to, &message).await {
                    self.undelivered(&speaker, &to, &error);
                }
                Ok(())
            }
            Letter::Left { requester } => {
                self.requester_left(Peer::new(&item.from, &requester));
                Ok(())
            }
        }
    }

    /// Tells a resident of another machine that its words to `to` could not be delivered here.
    fn undelivered(&self, speaker: &Peer, to: &str, error: &str) {
        let notice = match crate::town::core::delivery_id() {
            Ok(id) => Delivery {
                id,
                from: "world".into(),
                kind: "notice".into(),
                text: format!("=== message to {to} on {} (not delivered) ===\n{error}", self.machine()),
                as_tool_output: false,
            },
            Err(id_error) => return self.log(format!("cannot tell {} on {} that its words to {to} failed ({error}): {id_error}", speaker.id, speaker.machine)),
        };
        if let Err(mail_error) = self.mail(&speaker.machine, &Letter::Deliver { to: speaker.id.clone(), delivery: notice }) {
            self.log(format!("cannot tell {} on {} that its words to {to} failed ({error}): {mail_error}", speaker.id, speaker.machine));
        }
    }
}
