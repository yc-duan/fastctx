//! Native document frames are never coalesced: losing one requires a fresh
//! state-vector exchange. The WebView retains the replica and exact outbox.
use fastctx_client::composer::documents::{Documents, Input};
use fastctx_types::{composer::sync, ipc::FailureCode};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::atomic::Ordering,
    sync::{Arc, Mutex},
};
use tauri::ipc::Channel;

#[derive(Default, Clone)]
pub(crate) struct Inputs(Arc<Mutex<HashMap<u64, Input>>>);
impl Inputs {
    pub fn close(&self, id: u64) {
        self.0.lock().expect("composer inputs").remove(&id);
    }
    pub fn clear(&self) {
        self.0.lock().expect("composer inputs").clear();
    }
}
#[derive(Serialize)]
pub struct Watching {
    id: u64,
    watch: String,
    frame: sync::Frame,
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Native { watch: String, frame: sync::Frame },
    Interrupted { error: String, since: u64 },
}
fn unsupported(error: &fastctx_client::Error) -> bool {
    matches!(error, fastctx_client::Error::Call(fastctx_client::CallError::Failed(failure)) if failure.code == Some(FailureCode::Unsupported))
}
#[fastctx_gui_command::command]
pub async fn composer_watch_documents(
    shell: tauri::State<'_, crate::Shell>,
    resident_id: String,
    machine_name: Option<String>,
    subscription: sync::Subscription,
    pieces: Channel<crate::watch_delivery::Wake>,
) -> Result<Option<Watching>, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let resident = crate::resident(resident_id)?;
    let machine = crate::machine(machine_name)?;
    #[cfg(target_os = "android")]
    crate::android::readiness::wait(&world, machine.as_ref()).await?;
    let name = fastctx_client::composer::watch_id().map_err(|error| error.to_string())?;
    let mut documents =
        match world.watch_documents(resident.clone(), machine.clone(), name.clone(), subscription.clone()).await {
            Ok(documents) => documents,
            Err(error) if unsupported(&error) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
    let frame = match documents.next().await {
        Ok(Some(frame @ sync::Frame::State { mutations: true, .. })) => frame,
        Err(error) if unsupported(&error) => return Ok(None),
        Ok(Some(sync::Frame::State { mutations: false, .. })) => return Ok(None),
        Err(error) => return Err(error.to_string()),
        _ => return Err("the document stream ended before its state".into()),
    };
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    let inputs = shell.composer_inputs.clone();
    inputs.0.lock().expect("composer inputs").insert(id, documents.input().ok_or("the document stream is read-only")?);
    let mut initial = Some((documents, name.clone()));
    let cleanup = inputs.clone();
    shell
        .follow_delivery(
            page,
            id,
            pieces,
            |_| None,
            move |pieces, _| {
                let initial = initial.take();
                let (world, resident, machine, subscription, inputs) =
                    (world.clone(), resident.clone(), machine.clone(), subscription.clone(), inputs.clone());
                async move {
                    struct Closing(Inputs, u64);
                    impl Drop for Closing {
                        fn drop(&mut self) {
                            self.0.close(self.1);
                        }
                    }
                    let _closing = Closing(inputs.clone(), id);
                    let mut again = fastctx_client::Again::default();
                    let mut watching: Option<(Documents, String)> = initial;
                    #[cfg(target_os = "android")]
                    let mut visible = crate::android::visibility();
                    loop {
                        #[cfg(target_os = "android")]
                        if !*visible.borrow_and_update() {
                            watching = None;
                            inputs.close(id);
                            if pieces
                                .send(Piece::Interrupted {
                                    error: "the interface is in the background".into(),
                                    since: jiff::Timestamp::now().as_millisecond() as u64,
                                })
                                .is_err()
                            {
                                return;
                            }
                            if visible.changed().await.is_err() {
                                return;
                            }
                            continue;
                        }
                        if watching.is_none() {
                            let name = match fastctx_client::composer::watch_id() {
                                Ok(name) => name,
                                Err(_) => return,
                            };
                            match world
                                .watch_documents(resident.clone(), machine.clone(), name.clone(), subscription.clone())
                                .await
                            {
                                Ok(documents) => {
                                    let Some(input) = documents.input() else {
                                        return;
                                    };
                                    inputs.0.lock().expect("composer inputs").insert(id, input);
                                    watching = Some((documents, name));
                                }
                                Err(error) => {
                                    if pieces
                                        .send(Piece::Interrupted {
                                            error: error.to_string(),
                                            since: crate::epoch_ms(again.broke(error.to_string()).since),
                                        })
                                        .is_err()
                                    {
                                        return;
                                    }
                                    again.pause(&world, machine.as_ref()).await;
                                    continue;
                                }
                            }
                        }
                        let (documents, name) = watching.as_mut().expect("opened");
                        let next = tokio::select! {
                            next = documents.next() => next,
                            _ = crate::composer::shown(
                                #[cfg(target_os = "android")]
                                &mut visible,
                            ) => continue,
                        };
                        match next {
                            Ok(Some(frame)) => {
                                again.came();
                                if pieces.send(Piece::Native { watch: name.clone(), frame }).is_err() {
                                    return;
                                }
                            }
                            ended => {
                                let error = match ended {
                                    Err(error) => error.to_string(),
                                    _ => "the document stream ended".into(),
                                };
                                inputs.close(id);
                                watching = None;
                                if pieces
                                    .send(Piece::Interrupted {
                                        error: error.clone(),
                                        since: crate::epoch_ms(again.broke(error).since),
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                                again.pause(&world, machine.as_ref()).await;
                            }
                        }
                    }
                }
            },
        )
        .inspect_err(|_| {
            cleanup.close(id);
        })?;
    Ok(Some(Watching { id, watch: name, frame }))
}
#[fastctx_gui_command::command]
pub async fn composer_document_input(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    input: sync::Input,
) -> Result<(), crate::NotSaid> {
    let sender = shell.composer_inputs.0.lock().expect("composer inputs").get(&id).cloned();
    let sender = sender
        .ok_or_else(|| crate::NotSaid { again: true, message: "the document subscription is unavailable".into() })?;
    sender.send(input).map_err(|error| crate::NotSaid { again: true, message: error.to_string() })
}
