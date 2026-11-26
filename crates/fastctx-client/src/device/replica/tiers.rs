//! How much of each stream a device holds (design §29 3.3), by the kind of
//! the records it carries as the registry's `mobile` column has it
//! (`fastctx-sync` `registry/kinds.rs`): a stream is known by the name its
//! kind's records go under. A device holding whole takes every stream it
//! may hold by its frames from the first.

/// A device's tier: light, as a phone by default, or whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Light,
    Whole,
}

impl Tier {
    pub(super) fn code(self) -> u8 {
        match self {
            Self::Light => 0,
            Self::Whole => 1,
        }
    }

    pub(super) fn of_code(code: u8) -> Self {
        if code == 1 { Self::Whole } else { Self::Light }
    }

    /// The longest object a device of this tier fetches as its records
    /// come; longer ones as they are read. A whole device fetches all.
    pub fn fetches_at_most(self) -> u64 {
        fetched_at_most(self)
    }
}

/// How a light device holds a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hold {
    /// Followed whole, frame by frame.
    Complete,
    /// A window of the newest, frame by frame while it is close behind,
    /// older pages as they are read.
    Recent,
    /// Described only: what it holds is asked for as it is read.
    Metadata,
}

/// How many records behind the hub a recent stream may fall and still
/// follow it frame by frame; past it, a window.
pub(super) const RECENT_BEHIND: u64 = 200;

/// How many bytes of frames a recent stream may take to catch up before a
/// window is taken instead, whichever of this and `RECENT_BEHIND` comes
/// first (design §3.3, §28.2; codex-058517ed #15): the bytes a hub counts of
/// a frame, its header, signature and body.
pub(super) const RECENT_BYTES: u64 = 1 << 20;

/// How a device of `tier` holds the stream `stream`. A stream of a kind it
/// does not know is described only.
pub(super) fn hold(tier: Tier, stream: &str) -> Hold {
    if tier == Tier::Whole {
        return Hold::Complete;
    }
    // The registry's kind for each stream name, in brackets.
    const KINDS: &[(&str, Hold)] = &[
        ("meta", Hold::Complete),          // conversation.metadata
        ("words", Hold::Complete),         // conversation.words
        ("plan", Hold::Complete),          // conversation.plan
        ("history", Hold::Complete),       // conversation.history
        ("goals", Hold::Complete),         // conversation.goal
        ("sentinels", Hold::Complete),     // conversation.sentinel
        ("timers", Hold::Complete),        // conversation.timer
        ("draft", Hold::Complete),         // conversation.draft
        ("files", Hold::Complete),         // conversation.files
        ("settings", Hold::Complete),      // account.settings
        ("profile", Hold::Complete),       // account.profile
        ("collaboration", Hold::Complete), // account.collaboration
        ("notices", Hold::Complete),       // account.notice
        ("timeline", Hold::Recent),        // conversation.timeline
        ("jobs", Hold::Recent),            // job.facts
        ("audit", Hold::Recent),           // machine.audit
        ("activity", Hold::Recent),        // account.activity
        ("usage", Hold::Recent),           // account.usage
        ("node", Hold::Recent),            // machine.log
        ("mcp", Hold::Recent),             // mcp.log
        ("hub", Hold::Recent),             // hub.log
        ("sentinel-runs", Hold::Recent),   // conversation.sentinel-run
        ("attachments", Hold::Metadata),   // conversation.attachment
        ("artifacts", Hold::Metadata),     // conversation.artifact
        ("output", Hold::Metadata),        // job.output
        ("skills", Hold::Metadata),        // account.skill
        ("ledger", Hold::Metadata),        // project.ledger
    ];
    // The longest name the stream's begins with: `ledger/<project>`,
    // `usage/<machine>` and the like go by their kind's.
    KINDS
        .iter()
        .filter(|(name, _)| stream == *name || stream.strip_prefix(name).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|(name, _)| name.len())
        .map_or(Hold::Metadata, |(_, hold)| *hold)
}

/// The objects a device of `tier` fetches as its records come, at most this
/// long; longer ones as they are read. A whole device fetches all.
pub(super) fn fetched_at_most(tier: Tier) -> u64 {
    match tier {
        Tier::Light => 256 * 1024,
        Tier::Whole => u64::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_held_by_its_kind() {
        assert_eq!(hold(Tier::Light, "timeline"), Hold::Recent);
        assert_eq!(hold(Tier::Light, "usage/m1"), Hold::Recent);
        assert_eq!(hold(Tier::Light, "ledger/town"), Hold::Metadata);
        assert_eq!(hold(Tier::Light, "notices"), Hold::Complete);
        assert_eq!(hold(Tier::Light, "historyx"), Hold::Metadata, "a name that only begins as a kind's is not it");
        assert_eq!(hold(Tier::Light, "unknown"), Hold::Metadata);
        assert_eq!(hold(Tier::Whole, "timeline"), Hold::Complete);
    }
}
