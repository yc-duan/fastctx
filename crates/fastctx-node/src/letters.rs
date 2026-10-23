//! Large letters retain their exact JSON text across independently durable
//! transport frames. A recipient applies the reconstructed letter once.

mod delivered;
mod identity;
mod receiving;
pub(crate) use delivered::Handoff;
pub(crate) use identity::Identity;

pub(crate) use receiving::receive;

use std::io;
use std::sync::Mutex;

use fastctx_net::message::LetterId;

use fastctx_types::NoticeId;
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::remote::Letter;
use fastctx_types::residents::Notice;

use crate::Node;

static SENDING: Mutex<()> = Mutex::new(());

/// What a letter is sent for: words, or news of the sending machine
/// (`admission::letters`, which checks the same on the other side).
fn purpose(letter: &Letter) -> Purpose {
    match letter {
        Letter::Speak { .. } => Purpose::Words,
        Letter::Notice { notice, .. } if matches!(**notice, Notice::Words { .. } | Notice::Attached { .. }) => {
            Purpose::Words
        }
        Letter::Notice { notice, .. } if matches!(**notice, Notice::Sentinel { .. }) => Purpose::Echo,
        Letter::Left { .. } | Letter::Fragment { .. } | Letter::Notice { .. } | Letter::JobFacts { .. } => {
            Purpose::Echo
        }
    }
}

/// Sends `letter` to `to` for whom `acting` works, in parts when it does
/// not fit one message.
pub fn send(node: &Node, acting: &Acting, to: &Written, id: NoticeId, mut letter: Letter) -> io::Result<()> {
    let purpose = purpose(&letter);
    let (link, account) =
        node.speaking(acting, to, purpose).map_err(|failure| io::Error::other(failure.to_string()))?;
    if let Letter::Speak { accounts, .. } = &mut letter {
        *accounts = node.accounts_on(&link, acting).into_iter().filter(|other| *other != account).collect();
    }
    let to = &to.name;
    let _sending = SENDING.lock().expect("never poisoned");
    if link.letter_size(to, &account, &letter) <= fastctx_net::wire::MAX_CONTROL_FRAME {
        return link.send_letter(to, &account, purpose, LetterId::try_from(id.to_string()).expect("notice id"), letter);
    }
    let text = serde_json::to_string(&letter).map_err(io::Error::other)?;
    if text.len() > receiving::MAX_BYTES {
        return Err(io::Error::other(format!("the fragmented letter exceeds its {}-byte limit", receiving::MAX_BYTES)));
    }
    let mut rest = text.as_str();
    let mut parts = Vec::new();
    while !rest.is_empty() {
        // Maximum index/total encodings bound the final envelope's overhead.
        let fits = |end: usize| {
            link.letter_size(
                to,
                &account,
                &Letter::Fragment {
                    id: id.clone(),
                    index: usize::MAX,
                    total: usize::MAX,
                    text: rest[..end].to_string(),
                },
            ) <= fastctx_net::wire::MAX_CONTROL_FRAME
        };
        let boundaries: Vec<usize> = rest.char_indices().map(|(i, _)| i).chain(std::iter::once(rest.len())).collect();
        let (mut low, mut high) = (0, boundaries.len());
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            if fits(boundaries[middle]) { low = middle } else { high = middle }
        }
        let length = boundaries[low];
        if length == 0 {
            return Err(io::Error::other("the letter envelope leaves no room for one character"));
        }
        parts.push(rest[..length].to_string());
        rest = &rest[length..];
    }
    let total = parts.len();
    if total > receiving::MAX_PARTS {
        return Err(io::Error::other(format!("the letter needs more than {} fragments", receiving::MAX_PARTS)));
    }
    for (index, text) in parts.into_iter().enumerate() {
        let part = crate::speech::fixed_id("part", &[id.as_str(), &index.to_string()]);
        link.send_letter(
            to,
            &account,
            purpose,
            LetterId::try_from(part.to_string()).expect("notice id"),
            Letter::Fragment { id: id.clone(), index, total, text },
        )?;
    }
    Ok(())
}
