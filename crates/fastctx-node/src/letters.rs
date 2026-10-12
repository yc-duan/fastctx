//! Large letters retain their exact JSON text across independently durable
//! transport frames. A recipient applies the reconstructed letter once.

use std::io;
use std::sync::Mutex;

use fastctx_net::message::LetterId;
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::remote::Letter;
use fastctx_types::residents::Notice;
use fastctx_types::{MachineName, NoticeId};

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

pub fn receive(
    node: &Node,
    from: &MachineName,
    id: &NoticeId,
    index: usize,
    total: usize,
    text: &str,
) -> io::Result<Option<Letter>> {
    if total == 0 || index >= total {
        return Err(io::Error::other("invalid letter fragment numbering"));
    }
    let name = crate::speech::fixed_id("letter", &[from.key(), id.as_str()]);
    let directory = node.world.path().join("letter-parts").join(name.as_str());
    ensure_private_dir(&directory)?;
    let path = directory.join(index.to_string());
    let bytes = serde_json::to_vec(&(total, text)).map_err(io::Error::other)?;
    match publish_new(&path, &bytes, Permissions::Private) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::read(&path)? != bytes {
                return Err(io::Error::other("a letter fragment changed under the same id"));
            }
        }
        Err(error) => return Err(error),
    }
    let mut whole = String::new();
    for index in 0..total {
        let bytes = match std::fs::read(directory.join(index.to_string())) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let (count, text): (usize, String) = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if count != total {
            return Err(io::Error::other("letter fragments disagree about their total"));
        }
        whole.push_str(&text);
    }
    let letter: Letter = serde_json::from_str(&whole).map_err(io::Error::other)?;
    if matches!(letter, Letter::Fragment { .. }) {
        return Err(io::Error::other("nested letter fragments"));
    }
    Ok(Some(letter))
}
