//! Materializes and encodes one outgoing batch on a blocking worker.

use crate::{Hub, outq::Next};
use fastctx_net::{
    sign::PublicKey,
    wire::{self, Down},
};
use fastctx_types::MachineName;

type Encoded = (Vec<(Vec<u8>, bool)>, bool);

pub(super) fn prepare(hub: &Hub, name: &MachineName, key: PublicKey, next: Next) -> Result<Encoded, String> {
    let (frames, last) = match next {
        Next::Frames(frames) => (frames, false),
        Next::End(reason) => (vec![Down::Refused { reason }], true),
        Next::Overflowed(why) => return Err(why),
    };
    let mut encoded = Vec::with_capacity(frames.len());
    for mut frame in frames {
        if let Down::HistoryChanged { town, .. } = &frame {
            let store = hub.store();
            if store.indexed().member(name).is_none_or(|member| member.key != key)
                || !store.indexed().member_in_project(name, town)
            {
                continue;
            }
        }
        if let Down::AccountRecordChanged { changed } = &mut frame {
            let store = hub.store();
            let directory = store.indexed();
            if !crate::owned_records::hint(hub, &directory, name, &key, &changed.account, &changed.metadata) {
                continue;
            }
            if crate::record_subscriptions::replica_reader(hub, name, &key)
                && changed.metadata.by.as_ref().is_some_and(|by| {
                    !directory.member(name).is_some_and(|reader| {
                        reader.logins.iter().any(|account| {
                            directory.authorize(account, fastctx_net::authorization::Action::Work(by)).is_ok()
                        })
                    })
                })
            {
                changed.metadata.by = None;
            }
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
        encoded.push((bytes, dissolved));
    }
    Ok((encoded, last))
}
