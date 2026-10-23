//! The frozen protocol-5/6/7/8/9/10 subset used by already running tool adapters
//! and job supervisors. A GUI is never admitted through this compatibility path.
//! It can discover the mismatch and renew through the maintenance envelope.

use fastctx_types::ipc::Peer;
use fastctx_types::pages::PageCall;
use fastctx_types::residents::Notice;
use fastctx_types::run::{LegacyLines, OutputWindow, RunReply};
use fastctx_types::{Call, Reply};
use serde::{Serialize, Serializer};
use std::borrow::Cow;

#[path = "renewal_legacy/page_identity.rs"]
mod page_identity;

pub(crate) fn enabled(protocol: u32, peer: &Peer) -> bool {
    (protocol == 10 && matches!(peer, Peer::Command { .. }))
        || (protocol < fastctx_types::ipc::PROTOCOL && matches!(protocol, 5..=11) && self::peer(peer))
}

pub(crate) fn peer(peer: &Peer) -> bool {
    matches!(
        peer,
        Peer::Session { .. }
            | Peer::Supervisor { .. }
            | Peer::Command { resident: Some(_), .. }
            | Peer::Command { session: Some(_), .. }
    )
}

pub(crate) fn call(protocol: u32, peer: &Peer, call: &Call) -> bool {
    // Protocol 10 commands keep their OS-authenticated caller identity;
    // their optional receipt fields do not change command authorization.
    if protocol == 10 && matches!(peer, Peer::Command { .. }) {
        return true;
    }
    if protocol == 5 && matches!(call, Call::Machines { .. } | Call::Page(_)) {
        return false;
    }
    if let Peer::Supervisor { job: owned } = peer {
        return matches!(call, Call::JobChanged { job, .. } if owned == job);
    }
    matches!(
        call,
        Call::Arrive { .. }
            | Call::Run { .. }
            | Call::Write { .. }
            | Call::Copy { .. }
            | Call::Ask { .. }
            | Call::Message { .. }
            | Call::Collect { .. }
            | Call::Taken { .. }
            | Call::Who { .. }
            | Call::Look { .. }
            | Call::Saw { .. }
            | Call::Project(_)
            | Call::Journal(_)
            | Call::Nodes
            | Call::Machines { .. }
            | Call::Page(PageCall::Publish { .. } | PageCall::Read { .. } | PageCall::List { .. })
    )
}

pub(crate) fn reply(protocol: u32, reply: &mut Reply) {
    if protocol <= 10
        && let Reply::Notices { notices, .. } = reply
    {
        for delivery in notices {
            if let Notice::Answer { end, .. } = &mut delivery.notice {
                end.facts = None;
            }
        }
    }
    if protocol <= 10
        && let Reply::Machines { collab, .. } = reply
    {
        *collab = None;
    }
    if protocol <= 11 {
        if let Reply::Run(run) = reply {
            run.run = None;
        }
        if let Reply::Written { outcome } = reply {
            let changes = match outcome {
                Ok(result) => {
                    result.note_prefix = 0;
                    &mut result.changes
                }
                Err(error) => {
                    error.note_prefix = 0;
                    &mut error.changes
                }
            };
            for change in changes {
                change.fact = None;
            }
        }
    }
    // A callee may work for no account since protocol 12, on a computer in
    // no network; an older adapter reads every callee's account as a name,
    // so it is offered only those, and the colleague tools without any.
    if protocol <= 11
        && let Reply::Machines { collab, .. } = reply
        && let Some(offered) = collab
    {
        offered.callees.retain(|callee| callee.account.is_some());
        if offered.callees.is_empty() {
            *collab = None;
        }
    }
    // The World's own facts became a notice of their own in protocol 11;
    // before it they were the World's words, which is what the model reads.
    if protocol <= 10
        && let Reply::Notices { notices, .. } = reply
    {
        for delivery in notices {
            if let Notice::World { text, .. } = &delivery.notice {
                delivery.notice = Notice::Words {
                    from: fastctx_types::session::Speaker::World,
                    text: text.clone(),
                    goal_report: None,
                    goal_question: None,
                };
            }
        }
    }
    // Ends a run took away came with protocol 11.
    if protocol <= 10
        && let Reply::Run(run) = reply
    {
        run.consumed = None;
    }
    if protocol == 5
        && let Reply::Notices { notices, .. } = reply
    {
        for delivery in notices {
            let text = match &delivery.notice {
                Notice::Attached { message, text } => Some(message.render(text)),
                Notice::Words { from: fastctx_types::session::Speaker::Page { page }, text, .. } => {
                    Some(format!("=== from {page} ===\n{text}"))
                }
                _ => None,
            };
            if let Some(text) = text {
                delivery.notice = Notice::Words {
                    from: fastctx_types::session::Speaker::World,
                    text,
                    goal_report: None,
                    goal_question: None,
                };
            }
        }
    }
}

/// Serialize the contract of the reader, after execution, without changing
/// the node's own records or the replies sent to current clients.
pub(crate) struct Answer<'a> {
    pub protocol: u32,
    pub reply: &'a Reply,
}

impl Serialize for Answer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if matches!(self.protocol, 5 | 6)
            && let Reply::Run(run) = self.reply
        {
            let RunReply { machine, job, state, output, visited, .. } = run;
            let OutputWindow { log, bytes, lines, pieces, legacy, escaped_lines, .. } = output;
            let mut pieces = Cow::Borrowed(pieces.as_slice());
            let legacy = match legacy.as_slice() {
                [] => None,
                [one] => Some(one.clone()),
                many => {
                    // A rendered line can use several encodings. The old
                    // single-count field cannot express overlapping counts.
                    let encodings = many
                        .iter()
                        .map(|part| format!("{} ({} lines)", part.encoding, part.lines))
                        .collect::<Vec<_>>()
                        .join(", ");
                    pieces.to_mut().push(fastctx_types::run::OutputPiece::Text {
                        first_line: *lines,
                        text: format!("\n=== output decoded using {encodings}; counts may overlap on a line ===\n"),
                    });
                    None
                }
            };
            #[derive(Serialize)]
            struct Window<'a> {
                log: &'a str,
                bytes: u64,
                lines: u64,
                pieces: Cow<'a, [fastctx_types::run::OutputPiece]>,
                legacy: Option<LegacyLines>,
                escaped_lines: u64,
            }
            #[derive(Serialize)]
            struct Run<'a> {
                reply: &'static str,
                machine: &'a fastctx_types::MachineName,
                job: &'a fastctx_types::JobId,
                state: &'a fastctx_types::run::RunState,
                output: Window<'a>,
                visited: &'a [(fastctx_types::MachineName, fastctx_types::JobId)],
            }
            Run {
                reply: "run",
                machine,
                job,
                state,
                output: Window { log, bytes: *bytes, lines: *lines, pieces, legacy, escaped_lines: *escaped_lines },
                visited,
            }
            .serialize(serializer)
        } else {
            self.reply.serialize(serializer)
        }
    }
}

pub(crate) fn frame(protocol: u32, id: u64, reply: &Result<Reply, fastctx_types::ipc::Failure>) -> Vec<u8> {
    #[derive(Serialize)]
    struct Frame<'a> {
        kind: &'static str,
        id: u64,
        reply: Result<Answer<'a>, &'a fastctx_types::ipc::Failure>,
    }
    let reply = reply.as_ref().map(|reply| Answer { protocol, reply });
    let frame = Frame { kind: "reply", id, reply };
    if protocol < 10
        && frame.reply.as_ref().is_ok_and(|answer| {
            matches!(
                answer.reply,
                Reply::Controlled {
                    controlled: fastctx_types::session::control::Controlled::Reported { .. }
                        | fastctx_types::session::control::Controlled::Refused { .. }
                        | fastctx_types::session::control::Controlled::ModelMenu { .. },
                    ..
                }
            )
        })
    {
        let value = serde_json::to_value(&frame).expect("the World's types always serialize");
        let failure = fastctx_types::ipc::Failure::new(format!(
            "The call completed. This fastctx cannot display the host's control result. The complete result follows as JSON (do not repeat the call):\n{}",
            serde_json::to_string_pretty(&value["reply"]).expect("JSON serializes")
        ));
        return serde_json::to_vec(&Frame { kind: "reply", id, reply: Err(&failure) })
            .expect("the World's types always serialize");
    }
    // The machines hub by hub (`world-network.md`) have no shape in the
    // protocols before 8, which knew one World through one hub: the reply is
    // said whole, as text, rather than fitted into a shape that would
    // misstate it.
    if protocol < 8 && frame.reply.as_ref().is_ok_and(|answer| matches!(answer.reply, Reply::Nodes(_))) {
        let value = serde_json::to_value(&frame).expect("the World's types always serialize");
        let failure = fastctx_types::ipc::Failure::new(format!(
            "The call completed. This fastctx is older than the node and cannot show the machines hub by hub. The complete result follows as JSON (do not repeat the call):\n{}",
            serde_json::to_string_pretty(&value["reply"]).expect("JSON serializes")
        ));
        return serde_json::to_vec(&Frame { kind: "reply", id, reply: Err(&failure) })
            .expect("the World's types always serialize");
    }
    if protocol == 5 && frame.reply.as_ref().is_ok_and(|answer| page_identity::present(answer.reply)) {
        let value = serde_json::to_value(&frame).expect("the World's types always serialize");
        // Protocol 5 has no truthful value for a page author or owner.
        // An explicit textual result preserves attribution and effects;
        // inventing a resident or user would misattribute the operation.
        let failure = fastctx_types::ipc::Failure::new(format!(
            "The call completed. Protocol 5 cannot display page authors or job owners. The complete result follows as JSON (do not repeat the call):\n{}",
            serde_json::to_string_pretty(&value["reply"]).expect("JSON serializes")
        ));
        return serde_json::to_vec(&Frame { kind: "reply", id, reply: Err(&failure) })
            .expect("the World's types always serialize");
    }
    serde_json::to_vec(&frame).expect("the World's types always serialize")
}
