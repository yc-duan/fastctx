//! A terminal-only input probe. It never opens a World or starts an agent.
//! Pass an output JSONL path; Ctrl+Q exits and restores the console.

use std::fs::File;
use std::io::{BufWriter, Write};

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use fastctx_tui::input::{self, Input};
use serde_json::json;

struct Terminal;
impl Drop for Terminal {
    fn drop(&mut self) {
        for result in
            [input::restore(), crossterm::execute!(std::io::stdout(), DisableBracketedPaste), disable_raw_mode()]
        {
            if let Err(error) = result {
                eprintln!("terminal restoration failed: {error}");
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("usage: input_probe OUTPUT.jsonl")?;
    let mut file = BufWriter::new(File::create(path)?);
    enable_raw_mode()?;
    let terminal = Terminal;
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste)?;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    input::start(sender)?;
    println!("FASTCTX INPUT PROBE - Ctrl+Q exits\r");
    writeln!(file, "{}", json!({"ready": true, "pid": std::process::id()}))?;
    file.flush()?;
    while let Some(event) = receiver.blocking_recv() {
        let quit = matches!(event, Input::Key(key) if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::CONTROL);
        let failure = matches!(event, Input::Failed(_));
        let value = match event {
            Input::Key(key) => {
                json!({"key": format!("{:?}", key.code), "modifiers": format!("{:?}", key.modifiers), "kind": format!("{:?}", key.kind)})
            }
            Input::Paste(text) => json!({"paste": text}),
            Input::Pasting(count) => json!({"pasting": count}),
            Input::Resize => json!({"resize": true}),
            Input::Focus(focus) => json!({"focus": format!("{focus:?}")}),
            Input::Sequence(text) => json!({"sequence": text}),
            Input::Failed(error) => json!({"failed": error}),
        };
        writeln!(file, "{value}")?;
        file.flush()?;
        if failure {
            return Err("terminal input failed; see output log".into());
        }
        if quit {
            break;
        }
    }
    input::restore()?;
    drop(terminal);
    Ok(())
}
