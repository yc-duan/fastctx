//! Track only the inputs of sources that are currently being read.

use super::{dependencies, parts::Part};
use crate::Node;
use fastctx_types::{ResidentId, TownName, residents::About};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Default)]
pub(crate) struct Reading(Mutex<BTreeMap<ResidentId, Input>>);
struct Input {
    about: About,
    town: Option<TownName>,
    part: Part,
    signature: u64,
    changed: bool,
}
fn signature(node: &Node, about: &About, town: Option<&TownName>, part: Part) -> u64 {
    dependencies::Scan::default().observed_inputs(node, about, town)[part as usize]
}
impl Reading {
    pub(super) fn start<'a>(&'a self, node: &Node, about: &About, town: Option<&TownName>, part: Part) -> Guard<'a> {
        let mut inputs = self.0.lock().expect("context reads");
        inputs.insert(
            about.id.clone(),
            Input {
                about: about.clone(),
                town: town.cloned(),
                part,
                signature: signature(node, about, town, part),
                changed: false,
            },
        );
        Guard { reading: self, id: about.id.clone() }
    }
    pub(crate) fn changed(&self, node: &Node) {
        let mut inputs = self.0.lock().expect("context reads");
        let mut sources = dependencies::Scan::default();
        for input in inputs.values_mut().filter(|input| !input.changed) {
            input.changed = sources.observed_inputs(node, &input.about, input.town.as_ref())[input.part as usize]
                != input.signature;
        }
    }
}
pub(super) struct Guard<'a> {
    reading: &'a Reading,
    id: ResidentId,
}
impl Guard<'_> {
    pub(super) fn current(&self, node: &Node) -> bool {
        let inputs = self.reading.0.lock().expect("context reads");
        let input = inputs.get(&self.id).expect("reading source");
        !input.changed && signature(node, &input.about, input.town.as_ref(), input.part) == input.signature
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.reading.0.lock().expect("context reads").remove(&self.id);
    }
}
