//! Turn attribution for one decoded host row, independently of input visibility.
//!
//! Adapters supply semantics, not raw role names: tool results and command echoes
//! are not inputs merely because their host record uses the user role. This
//! decision allocates no identity and performs no IO. Persist the chosen turn in
//! the same source step as its declaration and row consumption.

/// A host row after the adapter has decoded its semantic role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowSemantics {
    /// A genuine accepted input, including a hidden World-framed prompt.
    /// `visible` controls only the input item, never turn allocation or tools.
    Input { visible: bool, boundary: InputBoundary },
    /// Turn-scoped material, including assistant output and tool calls/results.
    Conversation,
    /// Conversation metadata, such as a title; it does not consume a turn.
    Session,
    /// A host bookkeeping row with no timeline material.
    Ignored,
}

/// Evidence about an input's position in the host's execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputBoundary {
    /// No receipt yet. The native-order importer creates a bootstrap opening;
    /// a later strong binding may move that turn's items to an earlier turn.
    Unknown,
    /// A checked receipt identifies this input as an opening.
    Opens,
    /// A checked receipt identifies an append to an already running turn.
    Continues,
}

/// The native-order prefix already consumed by this host source.
/// Session metadata and ignored rows leave this context unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnContext {
    /// No turn-scoped material has appeared in this record yet.
    Empty,
    /// Material has appeared, but the first opening is still outside the prefix.
    Leading,
    /// At least one opening has been consumed, even if its input was hidden.
    Opened,
}

/// Where the row belongs before native bindings refine its attribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowChannel {
    Omitted,
    Session,
    /// One headless bootstrap turn preceding the first real opening. Its items
    /// use ordinary turn positions and do not require a fabricated Opening item.
    Leading,
    Turn,
}

/// Independent answers for turn allocation, input presentation and attribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowAttribution {
    pub opening_anchor: bool,
    /// Whether to emit the input item. A false value never suppresses tools,
    /// results or other conversation material from the same host row.
    pub show_input: bool,
    pub channel: RowChannel,
    /// Context to persist after consuming the row in native order.
    pub next: TurnContext,
}

/// Classifies a row without waiting for World evidence. Unknown genuine inputs
/// become bootstrap openings in native order; known appends do not. Material
/// before the first opening stays in a leading turn, including tool-result rows
/// with no parent. A title before that material remains in the Session channel.
///
/// Without a known binding, the importer creates the leading turn once on
/// `Empty -> Leading`, reuses it while `Leading`, and allocates subsequent
/// openings after it. A known binding can refine the target, but must not erase
/// the recorded native-order decision.
/// Opening items are optional: a hidden input can still be an opening anchor.
pub fn classify(row: RowSemantics, preceding: TurnContext) -> RowAttribution {
    let (opening_anchor, show_input, channel) = match row {
        RowSemantics::Input { visible, boundary } => {
            let opens = boundary != InputBoundary::Continues;
            (opens, visible, if opens { RowChannel::Turn } else { content_channel(preceding) })
        }
        RowSemantics::Conversation => (false, false, content_channel(preceding)),
        RowSemantics::Session => (false, false, RowChannel::Session),
        RowSemantics::Ignored => (false, false, RowChannel::Omitted),
    };
    let next = if opening_anchor {
        TurnContext::Opened
    } else if channel == RowChannel::Leading {
        TurnContext::Leading
    } else {
        preceding
    };
    RowAttribution { opening_anchor, show_input, channel, next }
}

fn content_channel(preceding: TurnContext) -> RowChannel {
    match preceding {
        TurnContext::Empty | TurnContext::Leading => RowChannel::Leading,
        TurnContext::Opened => RowChannel::Turn,
    }
}
