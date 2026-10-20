//! The rolling eight-day spending index, populated only by committed rows.
use fastctx_types::access::HubAccount;
use fastctx_types::usage::Usd;
use jiff::Timestamp;

pub(super) struct Charge {
    pub at: Timestamp,
    pub who: Option<HubAccount>,
    pub fable: bool,
    pub usd: Usd,
}
