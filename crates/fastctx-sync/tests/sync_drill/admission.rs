//! Waiting and cancellation must conserve byte permits and preserve FIFO order.
use fastctx_sync::{admission::ByteBudget, store::Error};
use std::{future::Future, task::Context, task::Waker, time::Duration};
use tokio::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(super) async fn run() -> Result<()> {
    let budget = ByteBudget::new(10)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut context = Context::from_waker(Waker::noop());
    let held = budget.try_acquire(6)?;
    {
        let mut head = Box::pin(budget.acquire_until(6, deadline));
        assert!(head.as_mut().poll(&mut context).is_pending());
        assert_eq!(budget.reserved(), 10, "the head reserved the remaining bytes");
        let mut tail = Box::pin(budget.acquire_until(1, deadline));
        assert!(tail.as_mut().poll(&mut context).is_pending());
        drop(head);
        let tail = tail.await?;
        assert_eq!(budget.reserved(), 7, "cancellation refunded the partial reservation");
        drop(tail);
    }
    drop(held);
    assert_eq!(budget.reserved(), 0);
    let held = budget.try_acquire(5)?;
    let last = budget.try_acquire(1)?;
    let mut head = Box::pin(budget.acquire_until(6, deadline));
    let mut tail = Box::pin(budget.acquire_until(1, deadline));
    assert!(head.as_mut().poll(&mut context).is_pending());
    assert!(tail.as_mut().poll(&mut context).is_pending());
    drop(last);
    assert!(tail.as_mut().poll(&mut context).is_pending(), "small requests cannot pass the head");
    assert!(matches!(budget.try_acquire(1), Err(Error::Busy)));
    drop(held);
    let head = head.await?;
    let tail = tail.await?;
    assert_eq!((head.bytes(), tail.bytes(), budget.reserved()), (6, 1, 7));
    drop((head, tail));
    assert_eq!(budget.reserved(), 0);
    let held = budget.try_acquire(10)?;
    assert!(matches!(budget.acquire_until(1, Instant::now()).await, Err(Error::Busy)));
    assert!(matches!(budget.acquire_until(1, Instant::now() + Duration::from_millis(1)).await, Err(Error::Busy)));
    drop(held);
    assert_eq!(budget.reserved(), 0);
    let held = budget.try_acquire(10)?;
    let mut waiting = Box::pin(budget.acquire_until(1, deadline));
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    budget.close();
    assert!(matches!(waiting.await, Err(Error::Unavailable(_))));
    drop(held);
    assert_eq!(budget.reserved(), 0);
    println!("byte admission: FIFO, partial cancellation, deadlines, close and permit release passed");
    Ok(())
}
