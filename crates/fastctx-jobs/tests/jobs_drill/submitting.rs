//! Calls that submit work, against a service that is only its jobs: what a
//! caller is told when the work is soon done and when it is not, that a
//! named call asked again is done once, and what holds the work back.

use crate::Outcome_;
use fastctx_jobs::Jobs;
use fastctx_jobs::submitted::{Family, Kept, Service, register, submit, withdraw};
use fastctx_types::ipc::Failure;
use fastctx_types::submission::Submission;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Lab {
    jobs: Arc<Jobs>,
    kept: Mutex<HashMap<[u8; 32], Vec<u8>>>,
    /// Whether the service takes work on: a node on its way out does not.
    serving: AtomicBool,
    named: AtomicUsize,
    /// How often what the work is given was made.
    made: AtomicUsize,
    /// How often each thing asked for was done.
    done: Mutex<HashMap<String, usize>>,
    /// Closed while work is to take long.
    gate: tokio::sync::Semaphore,
}

impl Service for Lab {
    fn told_as(&self) -> String {
        "the lab".into()
    }
    fn jobs(&self) -> &Arc<Jobs> {
        &self.jobs
    }
    fn fresh(&self) -> Result<String, String> {
        Ok(format!("unnamed-{}", self.named.fetch_add(1, Ordering::AcqRel)))
    }
    fn keep(&self, bytes: Vec<u8>) -> Kept<'_, [u8; 32]> {
        Box::pin(async move {
            let mut kept = self.kept.lock().expect("kept");
            let mut name = [0u8; 32];
            name[..8].copy_from_slice(&(kept.len() as u64 + 1).to_le_bytes());
            kept.insert(name, bytes);
            Ok(name)
        })
    }
    fn kept(&self, name: [u8; 32]) -> Kept<'_, Option<Vec<u8>>> {
        Box::pin(async move { Ok(self.kept.lock().expect("kept").get(&name).cloned()) })
    }
    fn begins(&self) -> Option<Box<dyn Send>> {
        self.serving.load(Ordering::Acquire).then(|| Box::new(()) as Box<dyn Send>)
    }
    fn log(&self, _: std::fmt::Arguments<'_>) {}
}

const QUICK: Family = Family {
    name: "call/quick",
    at_once: 4,
    within: Duration::from_secs(30),
    stays: Duration::from_secs(10),
    again: false,
};
/// Its work waits at the gate, and its callers stay a moment only.
const SLOW: Family = Family {
    name: "call/slow",
    at_once: 4,
    within: Duration::from_secs(30),
    stays: Duration::from_millis(200),
    again: false,
};

fn named(name: &str) -> Submission {
    Submission { name: name.into(), waits_ms: u32::MAX }
}

fn told(came: Result<String, Failure>) -> String {
    match came {
        Ok(answer) => format!("answered {} bytes", answer.len()),
        Err(failure) if failure.is_accepted() => "accepted".into(),
        Err(failure) => format!("failed: {}", failure.message),
    }
}

async fn ask(lab: &Lab, family: &Family, unit: &str, asked: &str, name: Option<&str>) -> Result<String, Failure> {
    let caller = name.map(named);
    let make = || async {
        lab.made.fetch_add(1, Ordering::AcqRel);
        Ok((unit.to_string(), asked))
    };
    submit(lab, family, asked.as_bytes(), caller.as_ref(), make, || {}).await
}

pub(crate) async fn drill(jobs: &Arc<Jobs>) -> Outcome_<()> {
    let lab = Arc::new(Lab {
        jobs: jobs.clone(),
        kept: Mutex::default(),
        serving: AtomicBool::new(true),
        named: AtomicUsize::new(0),
        made: AtomicUsize::new(0),
        done: Mutex::default(),
        gate: tokio::sync::Semaphore::new(0),
    });
    register(&lab, QUICK, |lab: Arc<Lab>, asked: String, _: Submission| async move {
        *lab.done.lock().expect("done").entry(asked.clone()).or_insert(0) += 1;
        Ok::<_, Failure>(format!("did {asked}"))
    });
    register(&lab, SLOW, |lab: Arc<Lab>, asked: String, _: Submission| async move {
        lab.gate.acquire().await.expect("the gate stays").forget();
        *lab.done.lock().expect("done").entry(asked.clone()).or_insert(0) += 1;
        if asked == "what fails" {
            return Err(Failure::refused("it may not be done"));
        }
        Ok::<_, Failure>(format!("did {asked}"))
    });
    let done = |asked: &str| lab.done.lock().expect("done").get(asked).copied().unwrap_or(0);
    let expect = |what: &str, came: String, wanted: &str| -> Outcome_<()> {
        if came.starts_with(wanted) { Ok(()) } else { Err(format!("{what}: {came}, not {wanted}").into()) }
    };

    // Work soon done is the call's answer, and a named call asked again is
    // answered from the one doing, under whatever unit it comes the second
    // time.
    let first = ask(&lab, &QUICK, "u1", "a thing", Some("n1")).await;
    let again = ask(&lab, &QUICK, "u2", "a thing", Some("n1")).await;
    // What the work is given is made for the operation, not for each asking.
    if first.as_deref().ok() != Some("did a thing")
        || again.as_deref().ok() != Some("did a thing")
        || done("a thing") != 1
        || lab.made.load(Ordering::Acquire) != 1
    {
        return Err(
            format!("a named call asked twice: {}, {}, done {}", told(first), told(again), done("a thing")).into()
        );
    }
    // The same name with another thing asked is another operation; a call
    // without a name is done each time it is asked.
    ask(&lab, &QUICK, "u1", "another thing", Some("n1")).await.map_err(|failure| failure.message)?;
    ask(&lab, &QUICK, "u1", "each time", None).await.map_err(|failure| failure.message)?;
    ask(&lab, &QUICK, "u1", "each time", None).await.map_err(|failure| failure.message)?;
    if done("another thing") != 1 || done("each time") != 2 {
        return Err("a name was taken for what was asked, or an unnamed call was done once for two askings".into());
    }
    // What was asked and what is answered are kept apart when they are large.
    let large = "x".repeat(200_000);
    let answer = ask(&lab, &QUICK, "u1", &large, Some("large")).await.map_err(|failure| failure.message)?;
    if answer.len() != large.len() + 4 || lab.kept.lock().expect("kept").len() != 2 {
        return Err("a large asking or its large answer was not kept apart and read back whole".into());
    }

    // Work that takes longer than its caller stays: a named caller is told
    // it is accepted, and asking again under the name says what came of it;
    // one without a name is told it is still being done.
    expect(
        "a named call whose work is slow",
        told(ask(&lab, &SLOW, "s1", "slow thing", Some("n2")).await),
        "accepted",
    )?;
    expect("asked again while it is done", told(ask(&lab, &SLOW, "s1", "slow thing", Some("n2")).await), "accepted")?;
    expect(
        "an unnamed call whose work is slow",
        told(ask(&lab, &SLOW, "s2", "unnamed slow", None).await),
        "failed: the lab did not finish call/slow within",
    )?;
    lab.gate.add_permits(2);
    let came = loop {
        match ask(&lab, &SLOW, "s1", "slow thing", Some("n2")).await {
            Err(failure) if failure.is_accepted() => tokio::time::sleep(Duration::from_millis(20)).await,
            came => break came,
        }
    };
    if came.as_deref().ok() != Some("did slow thing") || done("slow thing") != 1 {
        return Err(format!("a slow call asked again after: {}, done {}", told(came), done("slow thing")).into());
    }
    // A failure of the work is the answer to every asking, whole.
    lab.gate.add_permits(1);
    let failed = loop {
        match ask(&lab, &SLOW, "s3", "what fails", Some("n3")).await {
            Err(failure) if failure.is_accepted() => tokio::time::sleep(Duration::from_millis(20)).await,
            came => break came,
        }
    };
    let again = ask(&lab, &SLOW, "s3", "what fails", Some("n3")).await;
    if !failed.as_ref().is_err_and(Failure::is_refused)
        || !again.as_ref().is_err_and(Failure::is_refused)
        || done("what fails") != 1
    {
        return Err(format!("failed work asked again: {}, {}", told(failed), told(again)).into());
    }

    // A unit's work waits its turn; what has not begun can be taken back,
    // and whoever asks for it after is told it was not done.
    expect("the first of a unit", told(ask(&lab, &SLOW, "s4", "holds the unit", Some("n4")).await), "accepted")?;
    expect("the second of the unit", told(ask(&lab, &SLOW, "s4", "waits its turn", Some("n5")).await), "accepted")?;
    let withdrawn =
        withdraw(&*lab, &SLOW, "s4", "it was stopped before it began").await.map_err(|failure| failure.message)?;
    let taken_back = ask(&lab, &SLOW, "s4", "waits its turn", Some("n5")).await;
    lab.gate.add_permits(1);
    if withdrawn != 1 || !taken_back.as_ref().is_err_and(|failure| failure.not_run) || done("waits its turn") != 0 {
        return Err(format!("{withdrawn} withdrawn; asked after: {}", told(taken_back)).into());
    }

    // A service on its way out begins no work: the job waits, not begun,
    // for whoever serves next.
    lab.serving.store(false, Ordering::Release);
    expect(
        "a call while the service goes out",
        told(ask(&lab, &SLOW, "s5", "for the successor", Some("n6")).await),
        "accepted",
    )?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    if done("for the successor") != 0 {
        return Err("a service on its way out began work".into());
    }
    lab.serving.store(true, Ordering::Release);
    lab.gate.add_permits(1);
    let came = loop {
        match ask(&lab, &SLOW, "s5", "for the successor", Some("n6")).await {
            Err(failure) if failure.is_accepted() => tokio::time::sleep(Duration::from_millis(50)).await,
            came => break came,
        }
    };
    if came.as_deref().ok() != Some("did for the successor") || done("for the successor") != 1 {
        return Err(format!("work left for the successor: {}", told(came)).into());
    }
    println!(
        "submitting: answered when soon done, accepted when not, done once under a name, held back as it should be"
    );
    Ok(())
}
