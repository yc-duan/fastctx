//! The complete reply shares one budget, including everything delivered with it.

use std::io;
use std::path::PathBuf;

use fastctx_client::Client;
use fastctx_hands::answer::{cost, first_line_cost, fit};
use fastctx_hands::run::Content;
use fastctx_hands::run::present::display_path;
use fastctx_hands::run::window::{BUDGET, units};
use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::layout::WorldDir;
use fastctx_types::{Call, JobId, NoticeId, ResidentId};

/// What a tool call answers. The writer owns its receipt until stdout is flushed.
pub struct Answer {
    pub facts: Option<FactView>,
    pub content: Vec<Content>,
    pub is_error: bool,
    pub receipt: Option<(Client, ResidentId, Vec<NoticeId>)>,
    pub source: Option<String>,
    /// A job ended within its call whose whole output this answer shows,
    /// until the answer is shortened: told with the receipt, so the node
    /// removes what nobody reads again.
    pub shown: Option<JobId>,
    pub completed: Option<fastctx_types::run::CompletedJob>,
}

pub struct FactView {
    pub resident: ResidentId,
    pub call_id: String,
    pub facts: Box<fastctx_types::run_facts::RunFacts>,
    pub regions: Vec<fastctx_hands::run::present::ModelSpan>,
    pub full_output: fastctx_types::run_facts::OutputRef,
}

impl Answer {
    pub async fn keep_facts(&mut self, mut client: Client) -> Result<(), fastctx_client::CallError> {
        if let Some(view) = &mut self.facts {
            view.facts.spans = view.regions.iter().map(|region| region.span.clone()).collect();
            client
                .call(Call::StoreRunFacts {
                    resident: view.resident.clone(),
                    call_id: view.call_id.clone(),
                    facts: view.facts.clone(),
                })
                .await?;
        }
        Ok(())
    }

    fn fit(&mut self, budget: usize, note: &str) {
        let (content, kept) = fastctx_hands::answer::fit_mapped(&self.content, budget, note);
        self.content = content;
        if let Some(view) = &mut self.facts {
            view.regions = fastctx_hands::run::projection::remap(&view.regions, &kept);
            view.full_output.cut = true;
            view.facts.output = Some(view.full_output.clone());
        }
        self.shown = None;
    }
}

impl Answer {
    pub fn text(text: String, is_error: bool) -> Self {
        Self {
            facts: None,
            content: vec![Content::Text(text)],
            is_error,
            receipt: None,
            source: None,
            shown: None,
            completed: None,
        }
    }

    pub fn error(text: String) -> Self {
        Self::text(text, true)
    }

    /// Adds a block after what the answer holds, starting on a line of its own.
    pub fn follow(&mut self, text: String) {
        end_line(&mut self.content);
        self.content.push(Content::Text(text));
    }

    pub async fn sent(mut self) {
        if let Some((mut client, resident, ids)) = self.receipt.take()
            && let Err(error) = client
                .call(Call::Taken { resident, ids, shown: self.shown.take(), completed: self.completed.take() })
                .await
        {
            eprintln!("fastctx serve: the response was written but its receipt could not be recorded: {error}");
        }
    }

    /// Also covers errors returned before the resident could be identified.
    pub fn bound(&mut self, world: &WorldDir) -> io::Result<()> {
        if cost(&self.content) > BUDGET {
            let source = Source::for_answer(world, self);
            if units(&source.note) > BUDGET {
                return Err(io::Error::other("full-output location exceeds the answer budget"));
            }
            source.keep()?;
            self.fit(BUDGET, &source.note);
        }
        Ok(())
    }
}

/// A source is written only when shortening actually needs it. Run output
/// already has a durable source on the machine that executed the command.
pub struct Source {
    pub note: String,
    file: Option<(PathBuf, String)>,
}

impl Source {
    pub fn for_answer(world: &WorldDir, answer: &Answer) -> Self {
        match &answer.source {
            Some(source) => Self {
                note: format!("\n=== the answer stops here, at its limit; all of the output: {source} ===\n"),
                file: None,
            },
            None => Self::text(
                world,
                &answer
                    .content
                    .iter()
                    .map(|block| match block {
                        Content::Text(text) => text.as_str(),
                        Content::Image { .. } => unreachable!("images have their run log as source"),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        }
    }

    pub fn text(world: &WorldDir, text: &str) -> Self {
        let hash = fastctx_hands::files::content_hash(text.as_bytes());
        let path = world.path().join("out").join("answers").join(format!("{hash}.txt"));
        let note = format!(
            "\n=== the answer stops here, at its limit; all of it: {} on this machine ===\n",
            display_path(&path.to_string_lossy())
        );
        Self { note, file: Some((path, text.to_string())) }
    }

    pub fn keep(&self) -> io::Result<()> {
        if let Some((path, text)) = &self.file {
            ensure_private_dir(path.parent().expect("answer directory"))?;
            match publish_new(path, text.as_bytes(), Permissions::Private) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if std::fs::read(path)? != text.as_bytes() {
                        return Err(io::Error::other(format!("saved answer differs: {}", path.display())));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// Delivers notes first and then whole waiting messages in order. Only the
/// first message can be shortened; later messages wait for another answer.
/// Reserving the body's source notice prevents full outputs starving mail.
pub fn assemble(
    world: &WorldDir,
    answer: &mut Answer,
    mut notes: Vec<String>,
    leading_notes: usize,
    messages: impl IntoIterator<Item = (NoticeId, String)>,
) -> io::Result<Vec<NoticeId>> {
    if leading_notes > notes.len() {
        return Err(io::Error::other("the node's leading note count exceeds its notes"));
    }
    let trailing = notes.split_off(leading_notes);
    let source = Source::for_answer(world, answer);
    let note_cost = units(&source.note);
    let header = first_line_cost(&answer.content);
    let minimum = cost(&answer.content).min(if header + note_cost <= BUDGET { header + note_cost } else { note_cost });
    // The body may need one newline before whatever is delivered with it;
    // delivered texts end their own.
    let mut left =
        BUDGET.checked_sub(minimum + SEPARATOR).ok_or_else(|| io::Error::other("answer source exceeds its budget"))?;
    let mut additions = Vec::new();
    // Town notes share their acknowledgement, so they are shown together.
    if !notes.is_empty() {
        let text = ended(notes.join("\n"));
        additions.extend(shorten_text(world, text, left)?);
        left -= cost(&additions);
    }
    let note_blocks = additions.len();
    if !trailing.is_empty() {
        let blocks = shorten_text(world, ended(trailing.join("\n")), left)?;
        left -= cost(&blocks);
        additions.extend(blocks);
    }
    let mut ids = Vec::new();
    for (id, text) in messages {
        let text = ended(text);
        let size = units(&text);
        if size <= left {
            additions.push(Content::Text(text));
            left -= size;
        } else if ids.is_empty() {
            let saved = Source::text(world, &text);
            if units(&saved.note) > left {
                break;
            }
            saved.keep()?;
            let shown = fit(vec![Content::Text(text)], left, &saved.note);
            left -= cost(&shown);
            additions.extend(shown);
        } else {
            break;
        }
        ids.push(id);
    }
    let budget = BUDGET - SEPARATOR - cost(&additions);
    if cost(&answer.content) > budget {
        source.keep()?;
        answer.fit(budget, &source.note);
    }
    let messages = additions.split_off(note_blocks);
    insert_notes(answer, additions);
    if !messages.is_empty() {
        end_line(&mut answer.content);
    }
    answer.content.extend(messages);
    Ok(ids)
}

// One unit for splitting the header's text block, one for a separating newline.
const SEPARATOR: usize = 2;

fn insert_notes(answer: &mut Answer, notes: Vec<Content>) {
    if notes.is_empty() {
        return;
    }
    let count = notes.len();
    let split = match answer.content.first() {
        Some(Content::Text(text)) if text.starts_with("=== ") => Some(text.find('\n').map_or(text.len(), |at| at + 1)),
        _ => None,
    };
    let Some(split) = split else {
        if let Some(facts) = &mut answer.facts {
            for region in &mut facts.regions {
                region.block += count;
            }
        }
        answer.content.splice(0..0, notes);
        return;
    };
    let Content::Text(first) = &mut answer.content[0] else { unreachable!("text header") };
    let rest = first.split_off(split);
    let extra = usize::from(!rest.is_empty());
    if let Some(facts) = &mut answer.facts {
        for region in &mut facts.regions {
            if region.block == 0 {
                // Output spans exclude the run header; only their presentation moves.
                if let Some(bytes) = &mut region.bytes {
                    debug_assert!(bytes.start >= split);
                    bytes.start -= split;
                    bytes.end -= split;
                    region.block = count + 1;
                }
            } else {
                region.block += count + extra;
            }
        }
    }
    let mut inserted = notes;
    if extra != 0 {
        inserted.push(Content::Text(rest));
    }
    answer.content.splice(1..1, inserted);
}

/// Hosts show text blocks back to back, with nothing between them.
fn end_line(content: &mut [Content]) {
    if let Some(Content::Text(last)) = content.last_mut()
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
}

/// Each delivered text ends its own line, so the next block starts a new one.
fn ended(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn shorten_text(world: &WorldDir, text: String, budget: usize) -> io::Result<Vec<Content>> {
    if units(&text) <= budget {
        return Ok(vec![Content::Text(text)]);
    }
    let source = Source::text(world, &text);
    if units(&source.note) > budget {
        return Err(io::Error::other("full-text location exceeds the answer budget"));
    }
    source.keep()?;
    Ok(fit(vec![Content::Text(text)], budget, &source.note))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_hands::run::window::IMAGE_COST;

    #[test]
    fn complete_answer_budget_and_delivery_health() {
        let directory = tempfile::tempdir().unwrap();
        let world = WorldDir::under(directory.path());
        let header = "=== run on desk (exited 0) ===\n";
        let mut answer = Answer::text(format!("{header}{}", "😀中x".repeat(50_000)), false);
        answer.source = Some("desk C:/output.log".to_string());
        for _ in 0..28 {
            answer.content.push(Content::Image { mime: "image/png".into(), base64: "image".into() });
        }
        answer.content.push(Content::Text("the last output".into()));
        let first = NoticeId::try_from("first").unwrap();
        let second = NoticeId::try_from("second").unwrap();
        let words = "first message\n".to_string() + &"😀".repeat(40_000);
        let next = "second message\n".to_string() + &"中".repeat(40_000);
        let ids = assemble(
            &world,
            &mut answer,
            vec!["=== you are codex-1234 on desk ===".into()],
            1,
            [(first.clone(), words.clone()), (second.clone(), next.clone())],
        )
        .unwrap();
        assert_eq!(ids, [first]);
        assert!(cost(&answer.content) <= BUDGET);
        assert!(matches!(&answer.content[0], Content::Text(text) if text.starts_with(header)));
        assert!(answer.content.contains(&Content::Text(format!("{words}\n"))));
        assert!(!answer.content.contains(&Content::Text(format!("{next}\n"))));
        assert!(
            answer
                .content
                .iter()
                .any(|block| matches!(block, Content::Text(text) if text.contains("desk C:/output.log")))
        );
        assert_eq!(IMAGE_COST, 5_200);

        let mut reply = Answer::text("written".into(), false);
        assert_eq!(assemble(&world, &mut reply, Vec::new(), 0, [(second.clone(), next.clone())]).unwrap(), [second]);
        assert!(reply.content.contains(&Content::Text(format!("{next}\n"))));

        let huge = "=== colleague codex-5678 on desk ===\n".to_string() + &"😀中文".repeat(100_000);
        let id = NoticeId::try_from("huge").unwrap();
        let mut reply = Answer::text("written".into(), false);
        assert_eq!(assemble(&world, &mut reply, Vec::new(), 0, [(id.clone(), huge.clone())]).unwrap(), [id]);
        assert!(cost(&reply.content) <= BUDGET);
        let huge = format!("{huge}\n");
        let Source { file: Some((path, _)), .. } = Source::text(&world, &huge) else { unreachable!() };
        assert_eq!(std::fs::read_to_string(path).unwrap(), huge);

        let news = "=== the town changed ===\n".to_string() + &"new journal; ".repeat(40_000);
        let mut reply = Answer::text(format!("{header}done"), false);
        let waiting = NoticeId::try_from("waiting").unwrap();
        assert!(
            assemble(&world, &mut reply, vec![news.clone()], 1, [(waiting, "still waiting".into())])
                .unwrap()
                .is_empty()
        );
        assert!(cost(&reply.content) <= BUDGET);
        let news = format!("{news}\n");
        let Source { file: Some((path, _)), .. } = Source::text(&world, &news) else { unreachable!() };
        assert_eq!(std::fs::read_to_string(path).unwrap(), news);

        let mut error = Answer::error("😃".repeat(BUDGET + 1));
        error.bound(&world).unwrap();
        assert!(error.is_error);
        assert!((BUDGET - 2..=BUDGET).contains(&cost(&error.content)));
    }
}
