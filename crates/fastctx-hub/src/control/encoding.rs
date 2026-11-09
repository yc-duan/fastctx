//! Materializes and encodes one outgoing batch on a blocking worker.

use crate::{
    Hub,
    outq::Next,
    owned_records::hints::{self, Hint},
};
use fastctx_net::{
    sign::PublicKey,
    wire::{self, Down},
};
use fastctx_types::MachineName;
use tokio_tungstenite::tungstenite::Message;

use super::sending::{Guard, Outgoing};

type Encoded = (Vec<(Outgoing, bool)>, bool);

pub(super) fn prepare(hub: &Hub, name: &MachineName, key: PublicKey, next: Next) -> Result<Encoded, String> {
    let (frames, last) = match next {
        Next::Frames(frames) => (frames, false),
        Next::End(reason) => (vec![Down::Refused { reason }], true),
        Next::Overflowed(why) => return Err(why),
    };
    let mut encoded = Vec::with_capacity(frames.len());
    for mut frame in frames {
        let mut guard = None;
        if let Down::HistoryChanged { town, .. } = &frame {
            let store = hub.store();
            if store.indexed().member(name).is_none_or(|member| member.key != key)
                || !store.indexed().member_in_project(name, town)
            {
                continue;
            }
            guard = Some(Guard::Project(town.clone()));
        }
        if let Down::AccountRecordChanged { changed } = &mut frame {
            let store = hub.store();
            let directory = store.indexed();
            let hint = Hint::new(hub, &directory, name, &key, &changed.account, &changed.metadata);
            if !hint.allows(&directory, name, &key) {
                continue;
            }
            let writer = if hint.replica {
                changed.metadata.by.take().and_then(|by| {
                    if !hints::sees_writer(&directory, name, &by) {
                        return None;
                    }
                    let redacted = wire::encode(&Down::AccountRecordChanged { changed: changed.clone() });
                    changed.metadata.by = Some(by.clone());
                    Some((by, redacted))
                })
            } else {
                None
            };
            guard = Some(Guard::Record { hint: Box::new(hint), writer });
        }
        let bytes = wire::encode(&frame);
        if bytes.len() > wire::MAX_CONTROL_FRAME {
            return Err(format!(
                "outgoing control frame is {} bytes, more than {} the member reads; closing the session",
                bytes.len(),
                wire::MAX_CONTROL_FRAME
            ));
        }
        let dissolved = matches!(&frame, Down::Refused { reason } if crate::dissolve::tells(reason));
        encoded.push((Outgoing { message: Message::Binary(bytes.into()), guard }, dissolved));
    }
    Ok((encoded, last))
}
