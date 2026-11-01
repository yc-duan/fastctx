//! An on's step is acknowledged only after it is recorded. The short memory
//! lock protects its bounded tail; recording never holds that lock over IO.
use crate::Node;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fastctx_platform::doing::{MemoryMutex, Rank};
use fastctx_types::{
    JobId,
    ipc::Failure,
    on::{OnEnd, OnRequest},
    run_facts::{RunStep, StepDid, StepEnd, clipped, tail_of},
};
use std::{collections::VecDeque, io, path::PathBuf, sync::Arc};

const TAIL_LINES: usize = 20;
const TAIL_KEPT: usize = 16 * 1024;
pub(crate) type Steps = Arc<Telling>;
pub(crate) struct Telling {
    node: Arc<Node>,
    record: PathBuf,
    state: MemoryMutex<State>,
}
struct State {
    step: RunStep,
    tail: VecDeque<String>,
    kept: usize,
    ended: bool,
}

pub(crate) async fn begin(
    node: &Arc<Node>,
    parent: Option<&PathBuf>,
    request: &OnRequest,
) -> io::Result<Option<Steps>> {
    let Some(record) = parent.cloned() else { return Ok(None) };
    let Some(job) = request.job.clone() else { return Ok(None) };
    let script = request.package.script.clone();
    let command = fastctx_platform::task::spawn_blocking(move || fastctx_platform::disk::read_to_string(script))
        .await
        .map_err(io::Error::other)??;
    let id = crate::speech::new_id("on").to_string();
    let state = State {
        step: RunStep {
            id: id.clone(),
            job,
            machine: node.machine(),
            began: jiff::Timestamp::now(),
            ended: None,
            did: StepDid::On {
                to: request.machine.clone(),
                command: clipped(said(&command)),
                sent: None,
                end: None,
                pipes_to: request.pipe_to.as_ref().map(|written| written.name.clone()),
                fed_by: request.package.source.as_ref().map(|source| source.job.clone()),
                tail: None,
            },
        },
        tail: VecDeque::new(),
        kept: 0,
        ended: false,
    };
    Ok(Some(Arc::new(Telling {
        node: node.clone(),
        record,
        state: MemoryMutex::new(Rank::WRITE_FACTS, "on step tail", id, state),
    })))
}
fn said(command: &str) -> &str {
    let mut text = command.trim_end_matches(['\n', '\r']);
    while text.ends_with(' ') && !text.ends_with("\\ ") {
        text = &text[..text.len() - 1];
    }
    text
}
impl Telling {
    pub(crate) fn started(&self, sent: &JobId) -> io::Result<()> {
        let step = {
            let mut state = self.state.lock();
            if let StepDid::On { sent: known, .. } = &mut state.step.did {
                *known = Some(sent.clone());
            }
            state.step.clone()
        };
        crate::run_facts::tell_step(&self.node, &self.record, step)
    }
    pub(crate) fn output(&self, base64: &str) {
        if base64.is_empty() {
            return;
        }
        let from = base64.len().saturating_sub(TAIL_KEPT).div_ceil(4) * 4;
        let piece = &base64[from.min(base64.len())..];
        let mut state = self.state.lock();
        state.kept += piece.len();
        state.tail.push_back(piece.to_string());
        while state.kept > TAIL_KEPT {
            state.kept -= state.tail.pop_front().map_or(0, |piece| piece.len());
        }
    }
    pub(crate) fn ended(&self, end: &OnEnd) -> io::Result<()> {
        let end = match end {
            OnEnd::Exited { status, .. } => StepEnd::of(&fastctx_types::run::EndedHow::Finished, Some(*status)),
            OnEnd::Otherwise { how, status } => StepEnd::of(how, *status),
        };
        self.end(end)
    }
    pub(crate) fn failed(&self, failure: &Failure) -> io::Result<()> {
        let why = clipped(&failure.message);
        self.end(if failure.not_run { StepEnd::NotRun { why } } else { StepEnd::Lost { why } })
    }
    fn end(&self, end: StepEnd) -> io::Result<()> {
        let step = {
            let mut state = self.state.lock();
            if state.ended {
                return Ok(());
            }
            // A failed write retries the same outcome and timestamp.
            if state.step.ended.is_none() {
                let tail = state.tail_text();
                if let StepDid::On { end: known, tail: lines, .. } = &mut state.step.did {
                    *known = Some(end);
                    *lines = tail;
                }
                state.step.ended = Some(jiff::Timestamp::now());
            }
            state.step.clone()
        };
        crate::run_facts::tell_step(&self.node, &self.record, step)?;
        self.state.lock().ended = true;
        Ok(())
    }
}
impl State {
    fn tail_text(&self) -> Option<String> {
        let mut bytes = Vec::new();
        for piece in &self.tail {
            if let Ok(decoded) = STANDARD.decode(piece) {
                bytes.extend(decoded);
            }
        }
        let text = tail_of(&String::from_utf8_lossy(&bytes), TAIL_LINES);
        (!text.trim().is_empty()).then_some(text)
    }
}
