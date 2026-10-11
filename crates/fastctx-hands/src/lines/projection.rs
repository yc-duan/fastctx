//! Attribution shares the line index's canonical row assembly and byte checkpoints.

use super::*;
use crate::run::projection::CapturedNotes;
use fastctx_types::run_facts::OutputNote;

impl Index {
    /// Canonical UTF-8 pieces, cut additionally where a recorded note begins or ends.
    pub fn visit_projected(
        &mut self,
        start: u64,
        count: u64,
        cancelled: &AtomicBool,
        notes: &CapturedNotes,
        mut emit: impl FnMut(u64, String, bool, Option<OutputNote>) -> Result<(), String>,
    ) -> Result<u64, String> {
        let end = start.saturating_add(count).min(self.length);
        for number in start..end {
            check(cancelled)?;
            let row = self.row(number)?;
            let canonical = self.text(number, cancelled)?;
            let mut spans = Vec::new();
            if notes.intersects(row.start, row.end) {
                self.file.seek(SeekFrom::Start(row.start)).map_err(err)?;
                let mut input = (&mut self.file).take(row.end - row.start);
                let mut raw = Spool::default();
                let mut buffer = [0; BLOCK];
                loop {
                    check(cancelled)?;
                    let read = input.read(&mut buffer).map_err(err)?;
                    if read == 0 {
                        break;
                    }
                    raw.write_all(&buffer[..read]).map_err(err)?;
                }
                self.file_cursor = Some(row.end);
                if raw.len() != row.end - row.start {
                    return Err("output checkpoint ended during projection".into());
                }
                let mut lines = notes.lines(raw.finish().reader(), row.start);
                let mut ordinal = 0;
                let mut previous = None;
                let mut shown = Spool::default();
                while let Some(line) = lines.next_line().map_err(err)? {
                    check(cancelled)?;
                    if previous != Some(line.number) {
                        if previous.is_some() {
                            if ordinal == row.ordinal {
                                break;
                            }
                            ordinal += 1;
                        }
                        previous = Some(line.number);
                    }
                    if ordinal == row.ordinal {
                        let from = shown.len() as usize;
                        line.parts.write_text(&mut shown).map_err(err)?;
                        let to = shown.len() as usize;
                        if from < to
                            && let Some(note) = notes.note(line.view)
                        {
                            spans.push((from..to, note.clone()));
                        }
                    }
                }
                if !canonical.equals(&shown.finish()).map_err(err)? {
                    return Err("output attribution differs from the canonical indexed row".into());
                }
            }
            let mut offset = 0usize;
            canonical
                .utf8(|text| {
                    check(cancelled).map_err(io::Error::other)?;
                    let mut from = 0;
                    while from < text.len() {
                        let position = offset + from;
                        let at = spans.partition_point(|(range, _)| range.end <= position);
                        let (end, note) = match spans.get(at) {
                            Some((range, note)) if range.start <= position => {
                                ((range.end - offset).min(text.len()), Some(note.clone()))
                            }
                            Some((range, _)) => ((range.start - offset).min(text.len()), None),
                            None => (text.len(), None),
                        };
                        emit(number, text[from..end].into(), false, note).map_err(io::Error::other)?;
                        from = end;
                    }
                    offset += text.len();
                    Ok(())
                })
                .map_err(err)?;
            emit(number, String::new(), true, None)?;
        }
        Ok(end.saturating_sub(start))
    }
}
