//! The driver caller retries temporary resource/connection failures, never denials.
use fastctx_sync::store::Error;
use std::time::Duration;
#[derive(Default)]
pub(super) struct Counts {
    pub busy: u64,
    pub unavailable: u64,
}
pub(super) async fn request<T, F, Fut>(counts: &mut Counts, mut work: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Error>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        match work().await {
            Err(error @ Error::Busy) => {
                counts.busy += 1;
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
            }
            Err(error @ Error::Unavailable(_)) => {
                counts.unavailable += 1;
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
            }
            result => return result,
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
