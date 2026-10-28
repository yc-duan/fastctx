//! Consumer acknowledgements bound a watch without putting a round trip between
//! every pair of live events. A large history piece occupies the window alone.

use fastctx_types::ipc::Failure;
use std::{collections::VecDeque, time::Duration};

#[derive(Default)]
pub struct Flow {
    pending: VecDeque<(u64, usize)>,
    bytes: usize,
}
impl Flow {
    pub async fn ready(
        &mut self,
        bytes: usize,
        consumed: &mut tokio::sync::watch::Receiver<Option<u64>>,
    ) -> Result<(), Failure> {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(through) = *consumed.borrow_and_update() {
                    while self.pending.front().is_some_and(|(number, _)| *number <= through) {
                        self.bytes -= self.pending.pop_front().expect("acknowledged packet").1;
                    }
                }
                if self.pending.len() < 128
                    && (self.pending.is_empty() || self.bytes.saturating_add(bytes) <= 512 * 1024)
                {
                    return Ok(());
                }
                consumed.changed().await.map_err(|_| Failure::new("the watch consumer closed"))?;
            }
        })
        .await
        .map_err(|_| {
            Failure::new(
                "the watch consumer did not take its packets within 30 s; resume from the last received version",
            )
        })?
    }

    /// Records a packet only after its bytes have been written to the connection.
    pub fn sent(&mut self, number: u64, bytes: usize) {
        self.pending.push_back((number, bytes));
        self.bytes += bytes;
    }
}
