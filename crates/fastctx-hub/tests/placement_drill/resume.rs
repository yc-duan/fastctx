//! A viewer that comes back with the mark of the last event it took is sent
//! only the events after it; one that holds nothing yet is sent the segments
//! cut for the viewer before it and the events told since, its conversation's
//! history not read again.
use super::*;
use fastctx_client::Watching;
use fastctx_types::ResidentId;
use fastctx_types::session::held::{Held, SegmentId};
use fastctx_types::session::{Happened, Mark, Watched};

pub(super) async fn exercise(world: &World, host: &Path, resident: &ResidentId) -> Result<()> {
    let machine: Option<fastctx_types::access::Written> = Some("host".parse()?);
    let fresh = || Held { version: None, segments: Vec::new(), latest_first: false, since: None, marks: true };
    let mut first = world.watch_held(resident.clone(), machine.clone(), fresh()).await?;
    let (version, layout) = history(&mut first).await?;
    let before = last_turn(host, resident.as_str());
    world.talk(resident.clone(), machine.clone(), "RESUME_FIRST".into()).await?;
    let told = live(&mut first, 1).await?;
    record(
        "live events carry marks that count up from where the watch began",
        !told.is_empty() && counting(&told),
        json!(told.len()),
    );
    let last = *told.last().context("the first viewer was told nothing")?;
    drop(first);
    anyhow::ensure!(idle_after(host, resident.as_str(), before).await, "the watched turn did not settle");
    let before = last_turn(host, resident.as_str());
    world.talk(resident.clone(), machine.clone(), "RESUME_GAP".into()).await?;
    anyhow::ensure!(idle_after(host, resident.as_str(), before).await, "the turn no one watched did not settle");
    let held =
        Held { version: Some(version.clone()), segments: layout, latest_first: false, since: Some(last), marks: true };
    let mut back = world.watch_held(resident.clone(), machine.clone(), held).await?;
    let opened = tokio::time::timeout(std::time::Duration::from_secs(60), back.next()).await;
    let resumed = matches!(opened, Ok(Ok(Some(Watched::Resumed))));
    let missed = live(&mut back, 1).await?;
    record(
        "a viewer that comes back with its last mark is sent only the events after it",
        resumed
            && missed.first().is_some_and(|mark| mark.epoch == last.epoch && mark.sequence == last.sequence + 1)
            && counting(&missed),
        json!({"resumed": resumed, "missed": missed.len(), "after": last.sequence}),
    );
    drop(back);
    let mut new = world.watch_held(resident.clone(), machine.clone(), fresh()).await?;
    let (cut, _) = history(&mut new).await?;
    let since = live(&mut new, 2).await?;
    record(
        "a new viewer is sent the segments cut for the one before and the events since, the history not read again",
        cut == version && since.len() > told.len() && counting(&since),
        json!({"same cut": cut == version, "events since": since.len()}),
    );
    Ok(())
}

/// The version and layout of the history a watch sends first.
async fn history(watching: &mut Watching) -> Result<(String, Vec<SegmentId>)> {
    loop {
        let piece = tokio::time::timeout(std::time::Duration::from_secs(60), watching.next())
            .await
            .context("no history came within a minute")??;
        match piece {
            Some(Watched::Segments { version, layout, complete: true, .. }) => return Ok((version, layout)),
            Some(Watched::Unchanged { version }) => return Ok((version, Vec::new())),
            Some(_) => {}
            None => anyhow::bail!("the watch ended before its history"),
        }
    }
}

/// The marks of the live events a watch tells until `turns` turns have
/// ended, within two minutes.
async fn live(watching: &mut Watching, turns: usize) -> Result<Vec<Mark>> {
    let mut marks = Vec::new();
    let mut ended = 0;
    while ended < turns {
        let piece = tokio::time::timeout(std::time::Duration::from_secs(120), watching.next())
            .await
            .context("the turn's events did not come within two minutes")??;
        match piece {
            Some(Watched::Live { event, mark }) => {
                marks.extend(mark);
                if matches!(event.happened, Happened::TurnEnded { .. }) {
                    ended += 1;
                }
            }
            Some(_) => {}
            None => anyhow::bail!("the watch ended before the turn did"),
        }
    }
    Ok(marks)
}

/// Whether marks of one telling count up without going back.
fn counting(marks: &[Mark]) -> bool {
    marks.windows(2).all(|pair| pair[0].epoch == pair[1].epoch && pair[0].sequence < pair[1].sequence)
}
