//! What a resident reads back from `run`.
//!
//! The first line says where it ran and how it stands, and survives any
//! cut a host may make. Each head note has the form `=== subject (facts) ===`
//! and carries only facts the World is sure of and the reader cannot see
//! in the text itself (`world-view.md`, 输出长什么样).

use fastctx_types::run::{ExitStatus, OutputPiece, OutputWindow, RunReply, RunState};

use super::WAIT;

/// A piece of an answer as the host receives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image { mime: String, base64: String },
}

pub fn present(reply: &RunReply) -> Vec<Content> {
    let mut answer = Answer::default();
    answer.line(&format!("=== run on {} ({}) ===", reply.machine, facts(reply)));
    let output = &reply.output;
    let log = display_path(&output.log);
    let quoted_log = format!("'{}'", log.replace('\'', "'\"'\"'"));
    for piece in &output.pieces {
        match piece {
            OutputPiece::Text { text, .. } => answer.text(text),
            OutputPiece::PartOfLine { line, from, to, of, text } => {
                answer.line(&format!("=== line {line}: characters {from}-{to} of {of}; read with: fastctx view --lines {line}-{line} -- {quoted_log} ==="));
                answer.text(text);
            }
            OutputPiece::Omitted { first_line, last_line } => {
                let lines = if first_line == last_line {
                    format!("line {first_line}")
                } else {
                    format!("lines {first_line}-{last_line}")
                };
                answer.line(&format!(
                    "=== output from {lines} of {} not shown; read with: fastctx view --lines {first_line}-{last_line} -- {quoted_log} ===",
                    output.lines
                ));
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
            OutputPiece::Unreadable { line, reason } => answer.line(&format!("=== line {line}: {reason} ===")),
        }
    }
    match &reply.state {
        RunState::Running => {
            answer.line(&format!("=== {} goes on in the background; its output continues in {log} ===", reply.job))
        }
        RunState::Exited { remaining, .. } if *remaining > 0 => {
            answer.line(&format!("=== what it started goes on as {}; further output goes to {log} ===", reply.job))
        }
        _ => {}
    }
    answer.finish()
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
        RunState::Running => format!("still running after {}s; {}", WAIT.as_secs(), reply.job),
        RunState::Stopped => "stopped".to_string(),
        RunState::Interrupted { reason } => format!("interrupted: {reason}"),
        RunState::Failed { error } => format!("could not run: {error}"),
    }];
    facts.extend(output_facts(&reply.output));
    facts.join("; ")
}

fn output_facts(output: &OutputWindow) -> Vec<String> {
    let mut facts = Vec::new();
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
}

impl Answer {
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
        self.blocks.push(Content::Image { mime: mime.to_string(), base64: base64.to_string() });
    }

    fn finish(self) -> Vec<Content> {
        self.blocks
    }
}
