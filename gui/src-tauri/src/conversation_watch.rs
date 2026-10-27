//! Reopens a conversation after the page's delivery queue overflowed. The public
//! watch id stays the same; a fresh generation replaces any incomplete page view.

use crate::{NotWatched, Piece, transcript::Transcript, watch_delivery::Sender};
use fastctx_client::{Conversation, World};
use fastctx_types::{ResidentId, access::Written};
use std::sync::{Arc, Mutex};

pub(crate) async fn stream(
    initial: Option<Conversation>,
    world: World,
    resident: ResidentId,
    machine: Option<Written>,
    transcript: Arc<Mutex<Transcript>>,
    pieces: Sender<Piece>,
) {
    let conversation = match initial {
        Some(conversation) => conversation,
        None => {
            let mut again = fastctx_client::Again::default();
            loop {
                match open(&world, resident.clone(), machine.clone()).await {
                    Ok((conversation, history)) => {
                        let piece = transcript
                            .lock()
                            .expect("transcript")
                            .recovered(&history)
                            .unwrap_or_else(|error| Piece::Ended { error: Some(error) });
                        let ended = piece.last();
                        if pieces.send(piece).is_err() || ended {
                            return;
                        }
                        break conversation;
                    }
                    Err(error) => {
                        if error.deleted || error.refused {
                            let _ = pieces.send(error.piece());
                            return;
                        }
                        let interruption = again.broke(error.message);
                        if pieces.send(Piece::interrupted(&interruption)).is_err() {
                            return;
                        }
                        again.pause(&world, machine.as_ref()).await;
                    }
                }
            }
        }
    };
    #[cfg(target_os = "android")]
    crate::android::watching::stream(world, resident, machine, conversation, transcript, pieces).await;
    #[cfg(not(target_os = "android"))]
    {
        let mut conversation = conversation;
        loop {
            let update = match conversation.update().await {
                Ok(update) => update,
                Err(error) => {
                    let _ = pieces.send(Piece::failed(&error));
                    return;
                }
            };
            let piece = match transcript.lock().expect("transcript").take(update) {
                Ok(Some(piece)) => piece,
                Ok(None) => continue,
                Err(error) => Piece::Ended { error: Some(error) },
            };
            let ended = piece.last();
            if pieces.send(piece).is_err() || ended {
                return;
            }
        }
    }
}

async fn open(
    world: &World,
    resident: ResidentId,
    machine: Option<Written>,
) -> Result<(Conversation, Vec<fastctx_types::session::SessionEvent>), NotWatched> {
    #[cfg(target_os = "android")]
    {
        crate::android::watching::open(world, resident, machine, true).await
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(match machine {
            Some(machine) => Conversation::reopen(world, resident, machine).await,
            None => Conversation::watch(world, resident, None).await,
        }?)
    }
}
