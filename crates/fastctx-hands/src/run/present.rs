//! What a resident reads back from `run`.
//!
//! The first line says where it ran and how it stands, and survives any
//! cut a host may make. Each head note has the form `=== subject (facts) ===`
//! and carries only facts the World is sure of and the reader cannot see
//! in the text itself (`world-view.md`, 输出长什么样).

use fastctx_types::run::{Answered, ExitStatus, OutputPiece, OutputWindow, RunReply, RunState};

use super::WAIT;

/// A piece of an answer as the host receives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image { mime: String, base64: String },
}

pub fn present(reply: &RunReply) -> Vec<Content> {
    render(reply, false).0
}

#[derive(Clone, Debug)]
pub struct ModelSpan {
    pub block: usize,
    pub bytes: Option<std::ops::Range<usize>>,
    pub span: fastctx_types::run_facts::OutputSpan,
}

pub fn projected(reply: &RunReply) -> (Vec<Content>, Vec<ModelSpan>) {
    render(reply, true)
}

fn render(reply: &RunReply, project: bool) -> (Vec<Content>, Vec<ModelSpan>) {
    let mut answer = Answer::default();
    if project && let Some(facts) = &reply.run {
        answer.sources.extend(facts.spans.clone());
    }
    answer.line(&format!("=== run on {} ({}) ===", reply.machine, facts(reply)));
    let output = &reply.output;
    let log = display_path(&output.log);
    // What was left out is told as facts, where it is and how much; how to
    // read it is the model's to choose, so no command is put in its mouth.
    let mut partial = false;
    for piece in &output.pieces {
        match piece {
            OutputPiece::Text { text, .. } => answer.output_text(text),
            OutputPiece::PartOfLine { line, from, to, of, text } => {
                partial = true;
                answer.line(&format!("=== line {line}: characters {from}-{to} of {of} ==="));
                answer.output_text(text);
            }
            OutputPiece::Omitted { first_line, last_line } => {
                partial = true;
                let lines = if first_line == last_line {
                    format!("line {first_line}")
                } else {
                    format!("lines {first_line}-{last_line}")
                };
                answer.output_omitted(
                    &format!("=== {lines} of {} not shown ===", output.lines),
                    *first_line,
                    *last_line,
                );
            }
            OutputPiece::Image { mime, width, height, base64 } => {
                let caption = format!("(image {width}x{height}, {mime}) ===");
                if !answer.blocks.last().is_some_and(
                    |block| matches!(block, Content::Text(text) if text.trim_end_matches('\n').ends_with(&caption)),
                ) {
                    answer.line(&format!("=== image ({mime}, {width}x{height}) ==="));
                }
                answer.image(mime, base64);
            }
            OutputPiece::Unreadable { line, reason } => answer.output_line(&format!("=== line {line}: {reason} ===")),
        }
    }
    match &reply.state {
        RunState::Running => {
            answer.line(&format!("=== {} goes on in the background; all of its output is in {log} ===", reply.job))
        }
        RunState::Exited { remaining, .. } if *remaining > 0 => {
            answer.line(&format!("=== what it started goes on as {}; further output goes to {log} ===", reply.job))
        }
        _ if partial => answer.line(&format!("=== all of its output is in {log} ===")),
        _ => {}
    }
    // What macOS kept from it, as the node found: the user allows it there.
    if !reply.privacy.is_empty() {
        let places: Vec<String> = reply.privacy.iter().map(|place| display_path(place)).collect();
        answer.line(&format!(
            "=== macOS keeps {} private from Fastctx on {}; its user allows Fastctx in System Settings → Privacy & Security → Full Disk Access ===",
            places.join(", "),
            reply.machine
        ));
    }
    for missing in &reply.missing {
        let path = format!("$FASTCTX_SCRATCH/{}", missing.path);
        answer.line(&match &missing.why {
            None => format!(
                "=== {path} was still arriving on {} when this started; a later script finds it there ===",
                reply.machine
            ),
            Some(why) => format!("=== {path} could not be brought to {}: {why} ===", reply.machine),
        });
    }
    (answer.blocks, answer.regions)
}

/// The facts for the first line.
fn facts(reply: &RunReply) -> String {
    let mut facts = vec![match &reply.state {
        RunState::Ended { status } => status_text(*status),
        RunState::Exited { status, remaining } => {
            let processes = if *remaining == 1 {
                "1 process it started is".to_string()
            } else {
                format!("{remaining} processes it started are")
            };
            if *remaining == 0 {
                status_text(*status)
            } else {
                format!("{}; {processes} still running", status_text(*status))
            }
        }
        RunState::Running => match reply.answered {
            Some(Answered::Words { seconds }) => {
                format!("still running after {seconds}s, answered then because a message came for you; {}", reply.job)
            }
            // Asked not to wait at all.
            Some(Answered::WaitOver { seconds: 0 }) => format!("started; {}", reply.job),
            Some(Answered::WaitOver { seconds }) => format!("still running after {seconds}s; {}", reply.job),
            None => format!("still running after {}s; {}", WAIT.as_secs(), reply.job),
        },
        RunState::Stopped => "stopped".to_string(),
        RunState::Interrupted { reason } => format!("interrupted: {reason}"),
        RunState::Failed { error } => format!("could not run: {error}"),
    }];
    facts.extend(output_facts(&reply.output));
    if !reply.visited.is_empty() {
        let went: Vec<String> = reply.visited.iter().map(|(machine, job)| format!("{machine} {job}")).collect();
        facts.push(format!("on {}", went.join(", ")));
    }
    facts.join("; ")
}

fn output_facts(output: &OutputWindow) -> Vec<String> {
    let mut facts = Vec::new();
    // Whatever the shell's status: a script goes on past a command that
    // failed, and its output may bury what that command said.
    match output.failed.as_slice() {
        [] => {}
        [one] => facts.push(format!("{one} failed")),
        many => {
            let named: Vec<&str> = many.iter().take(3).map(String::as_str).collect();
            let more = if many.len() > 3 { format!(", and {} more", many.len() - 3) } else { String::new() };
            facts.push(format!("{} World commands failed: {}{more}", many.len(), named.join(", ")));
        }
    }
    for legacy in &output.legacy {
        let lines = if legacy.lines == 1 { "1 line".to_string() } else { format!("{} lines", legacy.lines) };
        facts.push(format!("{lines} decoded as {}", legacy.encoding));
    }
    if output.escaped_lines > 0 {
        let lines = if output.escaped_lines == 1 {
            "1 line has".to_string()
        } else {
            format!("{} lines have", output.escaped_lines)
        };
        facts.push(format!("{lines} undecodable bytes shown as \\xNN"));
    }
    facts
}

pub(crate) fn status_text(status: ExitStatus) -> String {
    status.to_string()
}

/// Paths in answers use forward slashes on every system (`world-run.md`).
pub fn display_path(path: &str) -> String {
    if cfg!(windows) { path.replace('\\', "/") } else { path.to_string() }
}

#[derive(Default)]
struct Answer {
    blocks: Vec<Content>,
    sources: std::collections::VecDeque<fastctx_types::run_facts::OutputSpan>,
    regions: Vec<ModelSpan>,
}

impl Answer {
    fn position(&self) -> (usize, usize) {
        match self.blocks.last() {
            Some(Content::Text(text)) => (self.blocks.len() - 1, text.len()),
            _ => (self.blocks.len(), 0),
        }
    }

    fn output_text(&mut self, text: &str) {
        let (block, mut at) = self.position();
        self.text(text);
        let mut remaining = text.len();
        while remaining > 0 {
            let Some(source) = self.sources.pop_front() else { break };
            let size = match &source.piece {
                fastctx_types::run_facts::DisplayPiece::Text { text, .. }
                | fastctx_types::run_facts::DisplayPiece::PartOfLine { text, .. } => text.len(),
                _ => panic!("output projection differs from its model text"),
            };
            let take = size.min(remaining);
            let span = super::projection::slice(&source, 0..take);
            self.regions.push(ModelSpan { block, bytes: Some(at..at + take), span });
            if take < size {
                self.sources.push_front(super::projection::slice(&source, take..size));
            }
            at += take;
            remaining -= take;
        }
    }

    fn output_line(&mut self, text: &str) {
        let (block, from) = self.position();
        self.line(text);
        let (_, to) = self.position();
        if let Some(span) = self.sources.pop_front() {
            self.regions.push(ModelSpan { block, bytes: Some(from..to), span });
        }
    }

    fn output_omitted(&mut self, text: &str, first: u64, last: u64) {
        let (block, from) = self.position();
        self.line(text);
        let (_, to) = self.position();
        while self.sources.front().is_some_and(|span| {
            matches!(span.piece,
                fastctx_types::run_facts::DisplayPiece::Omitted { first_line, last_line }
                if first_line >= first && last_line <= last
            )
        }) {
            let span = self.sources.pop_front().expect("omitted source");
            self.regions.push(ModelSpan { block, bytes: Some(from..to), span });
        }
    }
    /// A head note on a line of its own.
    fn line(&mut self, note: &str) {
        if let Some(Content::Text(open)) = self.blocks.last()
            && !open.is_empty()
            && !open.ends_with('\n')
        {
            self.text("\n");
        }
        self.text(note);
        self.text("\n");
    }

    fn text(&mut self, text: &str) {
        match self.blocks.last_mut() {
            Some(Content::Text(open)) => open.push_str(text),
            _ => self.blocks.push(Content::Text(text.to_string())),
        }
    }

    fn image(&mut self, mime: &str, base64: &str) {
        if let Some(span) = self.sources.pop_front() {
            self.regions.push(ModelSpan { block: self.blocks.len(), bytes: None, span });
        }
        self.blocks.push(Content::Image { mime: mime.to_string(), base64: base64.to_string() });
    }
}
