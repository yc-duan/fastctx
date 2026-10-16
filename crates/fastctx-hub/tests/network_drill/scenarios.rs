//! The scenarios on a hub of this release. Each puts one kind of trouble
//! on the network, checks that the trouble did happen, and checks what has
//! to hold however it went: every letter comes once, every call is answered
//! or given up and then stopped, every machine comes back.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fastctx_net::link::{BoxFuture, CallFailure, Feed};
use fastctx_net::wire::SILENCE;
use fastctx_types::remote::{RemoteCall, RemoteResult};
use fastctx_types::update::UpdateCall;
use fastctx_types::world::Path;

use crate::health;
use crate::lab::{self, IN_A_MESSAGE, OVER_DATA};
use crate::letters::{arrived, send};
use crate::report::{Report, seconds};
use crate::world::{World, until};

/// How long letters may take to arrive once the trouble is over.
const DRAIN: Duration = Duration::from_secs(180);
/// How long a machine may take to say a line gone dead is gone, past the
/// silence that says so.
const NOTICED: Duration = Duration::from_secs(15);

/// A receiver away while thousands of letters come for it: the hub holds
/// far more than one window of them, and sends them on once it is back.
pub async fn backlog(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let gamma = world.machine("gamma");
    gamma.stop();
    let mut sending = send(world, &["alpha", "beta"], "gamma", "backlog", 1500);
    sending.sent().await;
    let all = sending.wanted.len() as u64;
    let held = until(Duration::from_secs(90), || health::held(&world.hub).is_some_and(|held| held >= all)).await;
    report.check(
        held.is_some(),
        format!("the hub held all {all} letters for gamma while it was away, far more than one window"),
        format!("held {:?}", health::held(&world.hub)),
    );
    gamma.start()?;
    let came = arrived(&gamma.lab, &sending.wanted, DRAIN).await;
    report.check(
        came.missing.is_empty(),
        "every letter held reached gamma once it came back",
        format!("{} in {}", came.first_missing(), seconds(came.took)),
    );
    report.check(came.twice == 0, "none came twice", format!("{} twice", came.twice));
    Ok(())
}

/// Every connection cut again and again while letters are on their way.
pub async fn cuts(world: &World, report: &mut Report) -> anyhow::Result<()> {
    const CUTS: usize = 8;
    let gamma = world.machine("gamma");
    let before: Vec<usize> = world.machines.iter().map(|machine| machine.lab.connections()).collect();
    let mut sending = send(world, &["alpha", "beta"], "gamma", "cuts", 1500);
    let mut rng = crate::Rng(0x2545_f491_4f6c_dd1d);
    let mut amid = 0;
    for _ in 0..CUTS {
        tokio::time::sleep(Duration::from_millis(rng.between(400, 1600))).await;
        amid += usize::from(gamma.lab.got().len() < sending.wanted.len());
        for machine in &world.machines {
            machine.line.cut();
        }
    }
    sending.sent().await;
    let came = arrived(&gamma.lab, &sending.wanted, DRAIN).await;
    let again: Vec<usize> =
        world.machines.iter().zip(&before).map(|(machine, before)| machine.lab.connections() - before).collect();
    // A cut while a machine is still connecting again cuts nothing new, so
    // each connects again fewer times than it was cut.
    report.check(
        amid >= CUTS / 2 && again.iter().all(|again| *again >= 2),
        format!(
            "{CUTS} cuts of every connection, most while letters were on their way, and every machine connected again"
        ),
        format!("{amid} amid the letters; connected again {again:?} times"),
    );
    report.check(
        came.missing.is_empty(),
        "every letter reached gamma",
        format!("{} in {}", came.first_missing(), seconds(came.took)),
    );
    report.check(came.twice == 0, "none came twice", format!("{} twice", came.twice));
    Ok(())
}

/// The hub killed where it stands, again and again, while letters are on
/// their way.
pub async fn crash(world: &World, report: &mut Report) -> anyhow::Result<()> {
    const KILLS: usize = 5;
    let gamma = world.machine("gamma");
    let mut sending = send(world, &["alpha", "beta"], "gamma", "crash", 1500);
    let mut rng = crate::Rng(0x6a09_e667_f3bc_c908);
    let mut amid = 0;
    let mut pids = vec![world.hub.pid()];
    for _ in 0..KILLS {
        tokio::time::sleep(Duration::from_millis(rng.between(800, 2500))).await;
        amid += usize::from(gamma.lab.got().len() < sending.wanted.len());
        world.hub.restart(Duration::from_millis(rng.between(100, 1000))).await?;
        pids.push(world.hub.pid());
    }
    let mut distinct = pids.clone();
    distinct.sort_unstable();
    distinct.dedup();
    sending.sent().await;
    let came = arrived(&gamma.lab, &sending.wanted, DRAIN).await;
    report.check(
        world.hub.restarts.load(Ordering::SeqCst) == KILLS && distinct.len() == KILLS + 1 && amid >= KILLS / 2,
        format!(
            "the hub was killed {KILLS} times, most while letters were on their way, and each time a new process served"
        ),
        format!("{amid} amid the letters; processes {pids:?}"),
    );
    report.check(
        came.missing.is_empty(),
        "every letter reached gamma",
        format!("{} in {}", came.first_missing(), seconds(came.took)),
    );
    report.check(came.twice == 0, "none came twice", format!("{} twice", came.twice));
    Ok(())
}

/// The receiver stopped and started again, again and again, while letters
/// come for it.
pub async fn restart(world: &World, report: &mut Report) -> anyhow::Result<()> {
    const RESTARTS: usize = 6;
    let gamma = world.machine("gamma");
    let before = gamma.lab.connections();
    let mut sending = send(world, &["alpha", "beta"], "gamma", "restart", 1500);
    let mut rng = crate::Rng(0xbb67_ae85_84ca_a73b);
    let mut amid = 0;
    for _ in 0..RESTARTS {
        tokio::time::sleep(Duration::from_millis(rng.between(500, 2500))).await;
        amid += usize::from(gamma.lab.got().len() < sending.wanted.len());
        gamma.stop();
        tokio::time::sleep(Duration::from_millis(rng.between(500, 2000))).await;
        gamma.start()?;
    }
    sending.sent().await;
    let came = arrived(&gamma.lab, &sending.wanted, DRAIN).await;
    let again = gamma.lab.connections() - before;
    report.check(
        again >= RESTARTS && amid >= RESTARTS / 2,
        format!("gamma stopped and started {RESTARTS} times, most while letters came for it"),
        format!("{amid} amid the letters; connected {again} times"),
    );
    report.check(
        came.missing.is_empty(),
        "every letter reached gamma",
        format!("{} in {}", came.first_missing(), seconds(came.took)),
    );
    report.check(came.twice == 0, "none came twice", format!("{} twice", came.twice));
    Ok(())
}

/// The receiver's line gone dead without closing, both ways, for longer
/// than a silence, while letters come for it; then back. Meanwhile a
/// question the machine asked never answers: its caller gives it up at the
/// time it waits, and the machine asked stops it.
pub async fn freeze(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    beta.lab.stall(true);
    let who = RemoteCall::Who { all: true, town: None, since: None };
    let link = alpha.link();
    let (unanswered, ()) =
        tokio::join!(timed(link.call(&beta.name, &world.account, who.purpose(), who.clone())), async {
            if let Err(error) = dead_line(world, report, false).await {
                report.check(false, "the dead line's part ran to its end", format!("{error:#}"));
            }
        },);
    let (answer, took) = unanswered;
    let stopped = until(Duration::from_secs(15), || beta.lab.answering() == 0).await;
    beta.lab.stall(false);
    let deadline = who.contract().within.expect("a question waits so long");
    report.check(
        answer.is_err() && took >= deadline && took <= deadline + NOTICED,
        "a question beta never answered was given up at the time its caller waits",
        format!("{} after {}", said(&answer), seconds(took)),
    );
    report.check(
        stopped.is_some() && beta.lab.stopped() == 1,
        "and beta stopped it",
        format!("{} stopped", beta.lab.stopped()),
    );
    Ok(())
}

/// The receiver's line passing nothing from the hub while what it sends
/// still goes, for longer than a silence, while letters come for it: the
/// receiver hears nothing and takes it for gone, which ends it at the hub
/// too; then back.
pub async fn half_open(world: &World, report: &mut Report) -> anyhow::Result<()> {
    dead_line(world, report, true).await
}

/// Puts gamma's line dead, `one_way` from the hub only or both ways, while
/// letters come for it, and checks both ends take it for gone within a
/// silence, gamma comes back once its line does, and every letter comes.
async fn dead_line(world: &World, report: &mut Report, one_way: bool) -> anyhow::Result<()> {
    const FROZEN: Duration = Duration::from_secs(45);
    let gamma = world.machine("gamma");
    let tag = if one_way { "half-open" } else { "freeze" };
    let mut sending = send(world, &["alpha", "beta"], "gamma", tag, 1000);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = gamma.lab.connections();
    let froze = Instant::now();
    if one_way {
        gamma.line.freeze_to_machine(true);
    } else {
        gamma.line.freeze(true);
    }
    let (hub_saw, gamma_saw) = tokio::join!(
        world.sees("alpha", &gamma.name, false, SILENCE + NOTICED),
        until(SILENCE + NOTICED, || !gamma.online()),
    );
    report.check(
        hub_saw.is_some(),
        "the hub took gamma's dead line for gone within a silence, and alpha heard so",
        hub_saw.map_or("not within the time".to_string(), seconds),
    );
    report.check(
        gamma_saw.is_some(),
        "gamma took its dead line for gone within a silence too",
        gamma_saw.map_or("not within the time".to_string(), seconds),
    );
    tokio::time::sleep(FROZEN.saturating_sub(froze.elapsed())).await;
    gamma.line.freeze(false);
    let back = world.sees("alpha", &gamma.name, true, Duration::from_secs(90)).await;
    report.check(
        back.is_some() && gamma.lab.connections() > before,
        "gamma came back by itself once its line did",
        back.map_or("not within 90 s".to_string(), seconds),
    );
    sending.sent().await;
    let came = arrived(&gamma.lab, &sending.wanted, DRAIN).await;
    report.check(
        came.missing.is_empty(),
        "every letter reached gamma",
        format!("{} in {}", came.first_missing(), seconds(came.took)),
    );
    report.check(came.twice == 0, "none came twice", format!("{} twice", came.twice));
    Ok(())
}

/// A line so slow that one message takes longer than a silence to cross
/// it: it is not taken for a dead one, and what crosses it comes whole.
pub async fn slow(world: &World, report: &mut Report) -> anyhow::Result<()> {
    // Slow enough that what crosses takes well past a silence.
    const RATE: u64 = 1500;
    const STREAMED: usize = 80_000;
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    let link = alpha.link();
    let path = link.path_to(&beta.name);
    report.check(
        matches!(path, Path::Hub { .. }),
        "alpha's calls to beta go through the hub, so over beta's line",
        format!("{path:?}"),
    );
    // Any change of beta's link while the line is slow: its session ended.
    let mut status = beta.link().subscribe();
    status.mark_unchanged();
    beta.line.limit(RATE);
    let look = RemoteCall::Updates { update: UpdateCall::Look };
    let read = lab::read_of(STREAMED as u64);
    let streamed = Arc::new(AtomicUsize::new(0));
    let ((answer, took), (read_answer, read_took)) = tokio::join!(
        timed(link.call(&beta.name, &world.account, look.purpose(), look.clone())),
        timed(link.call_with_stream(
            &beta.name,
            &world.account,
            read.purpose(),
            read,
            taking(STREAMED, streamed.clone())
        )),
    );
    beta.line.limit(0);
    report.check(
        fails_with(&answer, |message| message.len() == IN_A_MESSAGE),
        format!(
            "an answer of {IN_A_MESSAGE} bytes in one control message came whole over a line of {RATE} bytes a second"
        ),
        format!("{} in {}", said(&answer), seconds(took)),
    );
    report.check(took >= SILENCE, "it took longer than a silence: the line was that slow", seconds(took));
    let got = streamed.load(Ordering::SeqCst);
    report.check(
        got == STREAMED && fails_with(&read_answer, |message| message == format!("sent {STREAMED}")),
        format!("{STREAMED} bytes over a data connection came whole over the same line"),
        format!("{got} bytes, {} in {}", said(&read_answer), seconds(read_took)),
    );
    report.check(read_took >= SILENCE, "they took longer than a silence too", seconds(read_took));
    let changed = status.has_changed().unwrap_or(true);
    report.check(
        !changed,
        "beta's slow line was never taken for a dead one",
        if changed { format!("{:?}", beta.link().status()) } else { String::new() },
    );
    Ok(())
}

/// Questions their caller gives up while the machine asked is answering
/// them: that machine is told, and stops them.
pub async fn cancel(world: &World, report: &mut Report) -> anyhow::Result<()> {
    const ASKED: usize = 20;
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    beta.lab.stall(true);
    let gave_up = give_up(world, "alpha", "beta", ASKED).await;
    let answering = beta.lab.answering();
    let stopped = until(Duration::from_secs(15), || beta.lab.answering() == 0).await;
    beta.lab.stall(false);
    report.check(
        gave_up == ASKED && answering == ASKED,
        format!("alpha gave up {ASKED} questions while beta was answering every one"),
        format!("gave up {gave_up}, beta answering {answering}"),
    );
    report.check(
        stopped.is_some() && beta.lab.stopped() == ASKED,
        "beta stopped them all once alpha gave them up",
        format!("{} stopped, {}", beta.lab.stopped(), stopped.map_or("not within 15 s".to_string(), seconds)),
    );
    report.check(alpha.online() && beta.online(), "both stayed connected", "");
    Ok(())
}

/// A machine leaving and coming back, three times: the others hear of
/// it at once; `deltas`, as each change, not only from whole directories.
pub async fn presence(world: &World, report: &mut Report, deltas: bool) -> anyhow::Result<()> {
    let gamma = world.machine("gamma");
    let (mut left, mut back) = (Vec::new(), Vec::new());
    for _ in 0..3 {
        gamma.stop();
        left.push(world.sees("alpha", &gamma.name, false, Duration::from_secs(15)).await);
        gamma.start()?;
        back.push(world.sees("alpha", &gamma.name, true, Duration::from_secs(30)).await);
    }
    let times = |seen: &[Option<Duration>]| {
        seen.iter().map(|took| took.map_or("never".to_string(), seconds)).collect::<Vec<_>>().join(", ")
    };
    report.check(left.iter().all(Option::is_some), "alpha saw gamma leave, each of three times", times(&left));
    report.check(back.iter().all(Option::is_some), "and come back, each time", times(&back));
    let told = world.machine("alpha").link().view().presence;
    if deltas {
        report.check(told > 0, "alpha's view counts the hub's changes of who is online", format!("change {told}"));
    } else {
        report.check(told == 0, "an earlier hub told no changes, and alpha heard of them all the same", "");
    }
    Ok(())
}

/// Answers of every size: within a control message, too large for one and
/// fetched over a data connection, and a megabyte streamed.
pub async fn answers(world: &World, report: &mut Report) -> anyhow::Result<()> {
    const STREAMED: usize = 1_000_000;
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    let link = alpha.link();
    let look = RemoteCall::Updates { update: UpdateCall::Look };
    let state = RemoteCall::Updates { update: UpdateCall::State };
    let read = lab::read_of(STREAMED as u64);
    let streamed = Arc::new(AtomicUsize::new(0));
    let ((small, small_took), (large, large_took), (read_answer, read_took)) = tokio::join!(
        timed(link.call(&beta.name, &world.account, look.purpose(), look.clone())),
        timed(link.call(&beta.name, &world.account, state.purpose(), state.clone())),
        timed(link.call_with_stream(
            &beta.name,
            &world.account,
            read.purpose(),
            read,
            taking(STREAMED, streamed.clone())
        )),
    );
    report.check(
        fails_with(&small, |message| message.len() == IN_A_MESSAGE),
        format!("an answer of {IN_A_MESSAGE} bytes came whole in a control message"),
        format!("{} in {}", said(&small), seconds(small_took)),
    );
    report.check(
        fails_with(&large, |message| message.len() == OVER_DATA),
        format!("an answer of {OVER_DATA} bytes came whole over a data connection"),
        format!("{} in {}", said(&large), seconds(large_took)),
    );
    let got = streamed.load(Ordering::SeqCst);
    report.check(
        got == STREAMED && fails_with(&read_answer, |message| message == format!("sent {STREAMED}")),
        format!("{STREAMED} bytes streamed came whole"),
        format!("{got} bytes, {} in {}", said(&read_answer), seconds(read_took)),
    );
    Ok(())
}

/// A stream open between two machines and quiet both ways, its far end's
/// line gone dead without closing: the near end takes the stream for gone
/// within a silence, though the hub still answers its pings.
pub async fn quiet_stream(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    let link = alpha.link();
    let read = lab::read_of(lab::QUIET);
    let (to, account, purpose) = (beta.name.clone(), world.account.clone(), read.purpose());
    let call = tokio::spawn(async move {
        let taken = Arc::new(AtomicUsize::new(0));
        timed(link.call_with_stream(&to, &account, purpose, read, taking(1, taken))).await
    });
    let open = until(Duration::from_secs(15), || beta.lab.answering() == 1).await;
    report.check(open.is_some(), "a quiet stream from beta to alpha was open", "");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let froze = Instant::now();
    beta.line.freeze(true);
    let ended = tokio::time::timeout(SILENCE + NOTICED, call).await;
    let took = froze.elapsed();
    beta.line.freeze(false);
    report.check(
        ended.is_ok(),
        "alpha took the quiet stream for gone within a silence once beta's line went dead, though the hub still answered its pings",
        seconds(took),
    );
    let stopped = until(Duration::from_secs(90), || beta.lab.answering() == 0).await;
    report.check(stopped.is_some(), "and beta let its end go once it came back", "");
    Ok(())
}

/// A stream whose receiving end takes nothing while it keeps saying it is
/// there: the end sending into it gives the write up within a silence or
/// so, and does not wait for ever.
pub async fn unread_stream(world: &World, report: &mut Report) -> anyhow::Result<()> {
    const SENT: u64 = 64 * 1024 * 1024;
    let (alpha, beta) = (world.machine("alpha"), world.machine("beta"));
    let link = alpha.link();
    let read = lab::read_of(SENT);
    let (to, account, purpose) = (beta.name.clone(), world.account.clone(), read.purpose());
    let began = Instant::now();
    let call = tokio::spawn(async move { link.call_with_stream(&to, &account, purpose, read, holding()).await });
    let gave_up = until(SILENCE * 3, || !beta.lab.reads().is_empty()).await;
    let reads = beta.lab.reads();
    report.check(
        gave_up.is_some() && reads.first().is_some_and(|(_, outcome)| !outcome.starts_with("sent")),
        "beta gave up sending to an end that took nothing though it kept saying it was there",
        reads.first().map_or(format!("still sending after {}", seconds(began.elapsed())), |(took, outcome)| {
            format!("{outcome}, after {}", seconds(*took))
        }),
    );
    let answered = tokio::time::timeout(Duration::from_secs(60), call).await;
    report.check(answered.is_ok(), "and alpha's call ended with it", "");
    Ok(())
}

/// Asks `to` from `from` `count` questions at once, each given up after two
/// seconds; how many were given up.
pub async fn give_up(world: &World, from: &str, to: &str, count: usize) -> usize {
    let link = world.machine(from).link();
    let to = world.machine(to).name.clone();
    let mut asks = Vec::new();
    for _ in 0..count {
        let (link, to, account) = (link.clone(), to.clone(), world.account.clone());
        asks.push(tokio::spawn(async move {
            let who = RemoteCall::Who { all: true, town: None, since: None };
            tokio::time::timeout(Duration::from_secs(2), link.call(&to, &account, who.purpose(), who)).await.is_err()
        }));
    }
    let mut gave_up = 0;
    for ask in asks {
        gave_up += usize::from(ask.await.unwrap_or(false));
    }
    gave_up
}

/// A call's answer and how long it took.
async fn timed<T>(call: impl std::future::Future<Output = T>) -> (T, Duration) {
    let began = Instant::now();
    let answer = call.await;
    (answer, began.elapsed())
}

/// A caller's end of a data connection that takes `bytes` and says it has
/// them, counting them into `taken`.
fn taking(bytes: usize, taken: Arc<AtomicUsize>) -> Feed {
    Box::new(move |mut stream| -> BoxFuture<Result<(), String>> {
        Box::pin(async move {
            stream
                .take(bytes as u64, |piece| {
                    taken.fetch_add(piece.len(), Ordering::SeqCst);
                    Ok(())
                })
                .await?;
            stream.done().await
        })
    })
}

/// A caller's end of a data connection that takes nothing, holding the
/// connection open as if it would.
fn holding() -> Feed {
    Box::new(move |stream| -> BoxFuture<Result<(), String>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_secs(600)).await;
            drop(stream);
            Ok(())
        })
    })
}

/// Whether the answer is the failure the lab gives, whose message `holds`.
fn fails_with(answer: &Result<RemoteResult, CallFailure>, holds: impl Fn(&str) -> bool) -> bool {
    matches!(answer, Ok(Err(failure)) if holds(&failure.message))
}

/// What an answer was, for the report.
fn said(answer: &Result<RemoteResult, CallFailure>) -> String {
    match answer {
        Ok(Err(failure)) => format!("a failure of {} bytes", failure.message.len()),
        Ok(Ok(_)) => "an answer".to_string(),
        Err(failure) => failure.to_string(),
    }
}
