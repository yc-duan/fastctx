//! Whole-window admission must conserve loans across cancellation and continuing IO.
use fastctx_sync::{admission::ByteBudget, replicate::Received, store::Error};
use std::{
    future::Future,
    task::{Context, Waker},
    time::Duration,
};
use tokio::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

pub(super) async fn run() -> Result<()> {
    loans().await?;
    fifo().await?;
    continuing_io().await?;
    shared_window().await?;
    metadata_reserve().await?;
    mixed().await?;
    println!("working windows: loans, FIFO, cancelled IO and 4096 mixed lifecycles conserved every byte");
    Ok(())
}

async fn loans() -> Result<()> {
    let budget = ByteBudget::new(32)?;
    let mut ordinary = budget.try_acquire(10)?;
    let split = ordinary.split(3)?;
    ordinary.shrink_to(6)?;
    assert_eq!(budget.reserved(), 9, "ordinary permits still refund shrinks globally");
    drop(split);
    assert_eq!(budget.reserved(), 6);
    drop(ordinary);
    assert_eq!(budget.reserved(), 0);
    let window = budget.window_until(24, deadline()).await?;
    assert!(budget.clone().owns_window(&window));
    assert!(!ByteBudget::new(32)?.owns_window(&window));
    let mut loan = window.take(16)?;
    let split = loan.split(6)?;
    loan.shrink_to(4)?;
    assert_eq!((window.capacity(), window.available(), budget.reserved()), (24, 14, 24));
    drop(split);
    assert_eq!(window.available(), 20);
    let reused = window.take(20)?;
    drop(reused);
    let body = Received::new(vec![1_u8; 4], loan);
    drop(window);
    assert_eq!(budget.reserved(), 4, "returned bodies outlive their window without becoming uncharged");
    assert_eq!(body.len(), 4);
    drop(body);
    assert_eq!(budget.reserved(), 0);

    let window = budget.window_until(16, deadline()).await?;
    let before = fastctx_platform::readings::counter("sync.budget_window_exhausted.integrity").get();
    eprintln!("checking the expected debug assertion for an insufficient working window");
    let failure = std::panic::catch_unwind(|| window.take(17));
    if cfg!(debug_assertions) {
        assert!(failure.is_err(), "debug cost-model mistakes assert at their origin");
    } else {
        let error = failure.expect("release returns a typed error").unwrap_err();
        assert_eq!((error.requested, error.available), (17, 16));
    }
    assert_eq!(fastctx_platform::readings::counter("sync.budget_window_exhausted.integrity").get(), before + 1);
    assert_eq!(window.available(), 16, "a failed take never consumes bytes or poisons the reservation");
    drop(window.take(16)?);
    drop(window);
    assert_eq!(budget.reserved(), 0);
    assert!(matches!(budget.window_until(33, deadline()).await, Err(Error::Invalid(_))));
    Ok(())
}

async fn fifo() -> Result<()> {
    let budget = ByteBudget::new(10)?;
    let held = budget.window_until(6, deadline()).await?;
    let mut context = Context::from_waker(Waker::noop());
    let mut head = Box::pin(budget.window_until(6, deadline()));
    let mut tail = Box::pin(budget.window_until(1, deadline()));
    assert!(head.as_mut().poll(&mut context).is_pending());
    assert!(tail.as_mut().poll(&mut context).is_pending());
    assert_eq!(budget.reserved(), 10);
    let loan = held.take(6)?;
    drop(held);
    assert!(tail.as_mut().poll(&mut context).is_pending(), "outstanding loans still belong to the head's predecessor");
    drop(head);
    let tail = tail.await?;
    assert_eq!(budget.reserved(), 7, "cancelling a queued window returns its partial reservation");
    drop((tail, loan));
    assert_eq!(budget.reserved(), 0);

    let held = budget.window_until(5, deadline()).await?;
    let last = budget.try_acquire(1)?;
    let mut head = Box::pin(budget.window_until(6, deadline()));
    let mut tail = Box::pin(budget.window_until(1, deadline()));
    assert!(head.as_mut().poll(&mut context).is_pending());
    assert!(tail.as_mut().poll(&mut context).is_pending());
    drop(last);
    assert!(tail.as_mut().poll(&mut context).is_pending(), "a smaller window cannot jump the FIFO head");
    drop(held);
    drop(head.await?);
    drop(tail.await?);
    assert_eq!(budget.reserved(), 0);

    let held = budget.window_until(6, deadline()).await?;
    assert!(matches!(budget.window_until(1, Instant::now()).await, Err(Error::Busy)));
    let mut expiring = Box::pin(budget.window_until(6, Instant::now() + Duration::from_secs(1)));
    assert!(expiring.as_mut().poll(&mut context).is_pending());
    assert_eq!(budget.reserved(), 10, "the expiring head actually owns a partial reservation");
    assert!(matches!(expiring.await, Err(Error::Busy)));
    assert_eq!(budget.reserved(), 6, "timeout returns the FIFO head's partial reservation");
    let mut waiting = Box::pin(budget.window_until(6, deadline()));
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    assert_eq!(budget.reserved(), 10);
    let loan = held.take(3)?;
    budget.close();
    assert!(matches!(waiting.await, Err(Error::Unavailable(_))));
    assert_eq!(budget.reserved(), 6, "close returns the FIFO head's partial reservation");
    drop(held);
    assert_eq!(budget.reserved(), 3, "closing admission does not release live IO");
    drop(loan);
    assert_eq!(budget.reserved(), 0);
    Ok(())
}

async fn continuing_io() -> Result<()> {
    let budget = ByteBudget::new(16)?;
    let window = budget.window_until(16, deadline()).await?;
    let io = window.take(7)?;
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        let _window = window;
        tokio::task::spawn_blocking(move || {
            let buffer = Received::new(vec![0_u8; 7], io);
            started.send(()).unwrap();
            blocked.recv().unwrap();
            assert_eq!(buffer.len(), 7);
            drop(buffer);
            finished.send(()).unwrap();
        })
        .await
        .unwrap();
    });
    ready.await?;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(budget.reserved(), 7, "cancelling the caller cannot refund its continuing blocking IO");
    let remaining = budget.window_until(9, deadline()).await?;
    assert_eq!(budget.reserved(), 16);
    assert!(matches!(budget.try_acquire(1), Err(Error::Busy)));
    drop(remaining);
    release.send(())?;
    done.await?;
    assert_eq!(budget.reserved(), 0);
    Ok(())
}

async fn mixed() -> Result<()> {
    let budget = ByteBudget::new(4096)?;
    let mut success = 0;
    let mut cancelled = 0;
    let mut errors = 0;
    for group in 0..32 {
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..128 {
            let budget = budget.clone();
            let id = group * 128 + index;
            let task = tasks.spawn(async move {
                let window = budget.window_until(64 + id % 193, deadline()).await.unwrap();
                let mut first = window.take(32).unwrap();
                let split = first.split(13).unwrap();
                tokio::task::yield_now().await;
                drop(split);
                first.shrink_to(7).unwrap();
                let second = window.take(window.available()).unwrap();
                if id % 5 == 0 {
                    return Err("simulated transfer failure");
                }
                drop(second);
                let body = Received::new(vec![0_u8; 7], first);
                drop(window);
                tokio::task::yield_now().await;
                Ok(body)
            });
            if id % 3 == 0 {
                task.abort();
            }
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(body)) => {
                    success += 1;
                    drop(body);
                }
                Ok(Err(_)) => errors += 1,
                Err(error) if error.is_cancelled() => cancelled += 1,
                Err(error) => return Err(error.into()),
            }
        }
        assert_eq!(budget.reserved(), 0, "every completed mixed batch returns to full capacity");
    }
    assert_eq!(success + cancelled + errors, 4096);
    assert!(success > 0 && cancelled > 0 && errors > 0);
    println!("working window mixed lifecycle totals: success={success}, cancelled={cancelled}, errors={errors}");
    Ok(())
}

async fn shared_window() -> Result<()> {
    let budget = ByteBudget::new(256)?;
    for _ in 0..32 {
        let window = std::sync::Arc::new(budget.window_until(256, deadline()).await?);
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(33));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..32 {
            let window = window.clone();
            let barrier = barrier.clone();
            tasks.spawn(async move {
                let mut loan = window.take(8).unwrap();
                drop(window);
                barrier.wait().await;
                let split = loan.split(3).unwrap();
                loan.shrink_to(2).unwrap();
                tokio::task::yield_now().await;
                drop((loan, split));
            });
        }
        barrier.wait().await;
        // Loan returns race the final owner dropping its idle reservation.
        drop(window);
        while let Some(result) = tasks.join_next().await {
            result?;
        }
        assert_eq!(budget.reserved(), 0);
    }
    Ok(())
}

async fn metadata_reserve() -> Result<()> {
    let budget = ByteBudget::new(32 * 1024)?;
    let ingress = budget.window_until(budget.capacity() / 8, deadline()).await?;
    let other_work = budget.window_until(20 * 1024, deadline()).await?;
    let gate = std::sync::Arc::new(tokio::sync::Barrier::new(5));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let raw = Received::new(vec![0_u8; 1024], ingress.take(1024)?);
        let gate = gate.clone();
        let budget = budget.clone();
        tasks.spawn(async move {
            gate.wait().await;
            // The entire decode working set is acquired before any typed buffer.
            let window = budget.window_until(8 * 1024, deadline()).await.unwrap();
            let scratch = window.take(3 * 1024).unwrap();
            let typed = window.take(5 * 1024).unwrap();
            assert_eq!(raw.len(), 1024);
            tokio::task::yield_now().await;
            drop((raw, scratch));
            let reply_scratch = window.take(3 * 1024).unwrap();
            drop((typed, reply_scratch, window));
        });
    }
    assert_eq!(ingress.available(), 0, "raw metadata filled its whole one-eighth reservation");
    gate.wait().await;
    // Even with other work still held, each decoder has room to finish without
    // waiting for another allocation while holding a partial working set.
    while let Some(result) = tasks.join_next().await {
        result?;
    }
    assert_eq!(ingress.available(), budget.capacity() / 8);
    drop((ingress, other_work));
    assert_eq!(budget.reserved(), 0);
    Ok(())
}
