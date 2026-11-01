//! A committed input stop finds its consuming turn even if binding already ended.
use crate::Node;
use fastctx_types::{NoticeId, ResidentId};
use std::{collections::BTreeMap, io, sync::Arc, time::Duration};

pub(super) fn notification(node: &Arc<Node>, resident: &ResidentId) -> Arc<dyn Fn() + Send + Sync> {
    let (runtime, node, resident) = (tokio::runtime::Handle::current(), node.clone(), resident.clone());
    Arc::new(move || schedule(&runtime, &node, &resident))
}

pub(super) fn schedule(runtime: &tokio::runtime::Handle, node: &Arc<Node>, resident: &ResidentId) {
    {
        let mut recovering = node.stops.recovering.lock().expect("input stop recovery");
        if let Some(dirty) = recovering.get_mut(resident) {
            *dirty = true;
            return;
        }
        recovering.insert(resident.clone(), false);
    }
    let (node, resident) = (node.clone(), resident.clone());
    runtime.spawn(async move {
        let mut problem = None;
        loop {
            if node.residents.about(&resident).is_none() {
                node.stops.recovering.lock().expect("input stop recovery").remove(&resident);
                return;
            }
            let result = super::store::ordered(node.world.conversation(&resident), |directory, _| {
                let mut turns = BTreeMap::<String, Vec<NoticeId>>::new();
                for input in super::store::pending(directory)? {
                    if let Some(turn) = crate::delivery::input_turn(directory, &input)? {
                        turns.entry(turn).or_default().push(input);
                    }
                }
                Ok::<_, io::Error>(turns)
            })
            .await;
            match result {
                Ok(turns) => {
                    for (turn, inputs) in turns {
                        super::deliver(&node, &resident, &turn, inputs);
                    }
                }
                Err(error) => {
                    let error = error.to_string();
                    if problem.as_ref() != Some(&error) {
                        node.log.line(format_args!("recovering {resident}'s input stops: {error}"));
                        problem = Some(error);
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            }
            let mut recovering = node.stops.recovering.lock().expect("input stop recovery");
            if recovering.get_mut(&resident).is_some_and(std::mem::take) {
                continue;
            }
            recovering.remove(&resident);
            return;
        }
    });
}
