//! Native receipts need revisions and delivery results, never a legacy text
//! projection. Only the legacy entry point materializes that optional view.
use super::{Stored, Updated};
use fastctx_types::{NoticeId, composer::Composer, residents::Landed};

pub(super) struct Answer {
    pub revision: u64,
    composer: Option<Composer>,
    pub landed: Option<Landed>,
    pub delivery: Option<NoticeId>,
}
impl Answer {
    pub fn state(state: &Stored, native: bool, landed: Option<Landed>, delivery: Option<NoticeId>) -> Self {
        Self { revision: state.revision, composer: (!native).then(|| state.view()), landed, delivery }
    }
    pub fn legacy(updated: Updated) -> Self {
        Self {
            revision: updated.composer.revision,
            composer: Some(updated.composer),
            landed: updated.landed,
            delivery: updated.delivery,
        }
    }
    pub fn updated(self) -> Updated {
        Updated {
            composer: self.composer.expect("legacy request retained its projection"),
            landed: self.landed,
            delivery: self.delivery,
        }
    }
}
