//! What an `on` did, as a step of the run whose script gave it
//! (`fastctx_types::run_facts::RunStep`): told by the node that carries it,
//! once when the job there started or could not, and once when it ended,
//! with the last lines it sent back.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_types::JobId;
use fastctx_types::ipc::Failure;
use fastctx_types::on::{OnEnd, OnRequest};
use fastctx_types::run_facts::{RunStep, StepDid, StepEnd, clipped, tail_of};

use crate::Node;

/// The lines of its output a step keeps.
const TAIL_LINES: usize = 20;
/// The most of its output, as base64, kept to find them in.
const TAIL_KEPT: usize = 16 * 1024;

pub(crate) type Steps = Arc<Mutex<Telling>>;

pub(crate) struct Telling {
    node: Arc<Node>,
    /// The record of the job whose script gave the `on`.
    record: PathBuf,
    step: RunStep,
    /// The last of what came back, as it came, base64.
    tail: VecDeque<String>,
    kept: usize,
    ended: bool,
}

/// The telling of `request`, an `on` given in the job whose record is
/// `parent`; none for an `on` no run gave.
pub(crate) fn begin(node: &Arc<Node>, parent: Option<&PathBuf>, request: &OnRequest) -> Option<Steps> {
    let record = parent?.clone();
    let job = request.job.clone()?;
    let command = std::fs::read_to_string(&request.package.script).unwrap_or_default();
    let id = crate::speech::new_id("on").to_string();
    Some(Arc::new(Mutex::new(Telling {
        node: node.clone(),
        record,
        step: RunStep {
            id,
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
    })))
}

/// The command line as it reads: bash's quoting leaves a space after the
/// last word, kept only where it is the word's own.
fn said(command: &str) -> &str {
    let mut text = command.trim_end_matches(['\n', '\r']);
    while text.ends_with(' ') && !text.ends_with("\\ ") {
        text = &text[..text.len() - 1];
    }
    text
}

impl Telling {
    /// The job there started as `sent`.
    pub(crate) fn started(&mut self, sent: &JobId) {
        if let StepDid::On { sent: known, .. } = &mut self.step.did {
            *known = Some(sent.clone());
        }
        self.tell();
    }

    /// Output it sent back, base64 as it came.
    pub(crate) fn output(&mut self, base64: &str) {
        self.kept += base64.len();
        self.tail.push_back(base64.to_string());
        while self.kept > TAIL_KEPT && self.tail.len() > 1 {
            self.kept -= self.tail.pop_front().map_or(0, |piece| piece.len());
        }
    }

    pub(crate) fn ended(&mut self, end: &OnEnd) {
        let end = match end {
            OnEnd::Exited { status, .. } => StepEnd::of(&fastctx_types::run::EndedHow::Finished, Some(*status)),
            OnEnd::Otherwise { how, status } => StepEnd::of(how, *status),
        };
        self.end(end);
    }

    pub(crate) fn failed(&mut self, failure: &Failure) {
        let why = clipped(&failure.message);
        self.end(if failure.not_run { StepEnd::NotRun { why } } else { StepEnd::Lost { why } });
    }

    fn end(&mut self, end: StepEnd) {
        if self.ended {
            return;
        }
        self.ended = true;
        let tail = self.tail_text();
        if let StepDid::On { end: known, tail: lines, .. } = &mut self.step.did {
            *known = Some(end);
            *lines = tail;
        }
        self.step.ended = Some(jiff::Timestamp::now());
        self.tell();
    }

    /// Its last lines, from the end of what was kept.
    fn tail_text(&self) -> Option<String> {
        let mut bytes = Vec::new();
        for piece in &self.tail {
            // A suffix at a multiple of four characters is the encoding of
            // a suffix of the bytes.
            let from = piece.len().saturating_sub(TAIL_KEPT) / 4 * 4;
            if let Ok(decoded) = STANDARD.decode(&piece[from..]) {
                bytes.extend(decoded);
            }
        }
        let text = tail_of(&String::from_utf8_lossy(&bytes), TAIL_LINES);
        (!text.trim().is_empty()).then_some(text)
    }

    fn tell(&self) {
        if let Err(error) = crate::run_facts::tell_step(&self.node, &self.record, self.step.clone()) {
            self.node.log.line(format_args!("telling the step of an on to {}: {error}", self.record.display()));
        }
    }
}
