//! Public tokenUsage snapshots are cumulative per thread, not per turn.
//! Rate-limit notifications repeat them; compaction only re-estimates last.

use std::collections::HashMap;

use fastctx_types::session::{Happened, SessionChange, SessionEvent, TurnFacts, TurnTokens};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Counts {
    input: u64,
    cached: u64,
    cache_write: Option<u64>,
    output: u64,
    reasoning: u64,
    total: u64,
}

impl Counts {
    fn read(value: &Value) -> Option<Self> {
        Some(Self {
            input: value["inputTokens"].as_u64()?,
            cached: value["cachedInputTokens"].as_u64()?,
            cache_write: value["cacheWriteInputTokens"].as_u64(),
            output: value["outputTokens"].as_u64()?,
            reasoning: value["reasoningOutputTokens"].as_u64()?,
            total: value["totalTokens"].as_u64()?,
        })
    }

    fn difference(&self, previous: &Self) -> Option<Self> {
        Some(Self {
            input: self.input.checked_sub(previous.input)?,
            cached: self.cached.checked_sub(previous.cached)?,
            cache_write: self.cache_write.zip(previous.cache_write).and_then(|(a, b)| a.checked_sub(b)),
            output: self.output.checked_sub(previous.output)?,
            reasoning: self.reasoning.checked_sub(previous.reasoning)?,
            total: self.total.checked_sub(previous.total)?,
        })
    }

    fn add(&mut self, other: Self) {
        let add = |a: u64, b: u64| a.checked_add(b).expect("Codex turn token count overflow");
        self.input = add(self.input, other.input);
        self.cached = add(self.cached, other.cached);
        self.cache_write = self.cache_write.zip(other.cache_write).map(|(a, b)| add(a, b));
        self.output = add(self.output, other.output);
        self.reasoning = add(self.reasoning, other.reasoning);
        self.total = add(self.total, other.total);
    }

    fn synthetic_full(&self, window: Option<u64>) -> bool {
        // Codex 740e5af protocol.rs::fill_to_context_window resets all
        // components to zero while filling total with the context window.
        self.total > 0
            && window == Some(self.total)
            && self.input == 0
            && self.cached == 0
            && self.cache_write.is_none_or(|count| count == 0)
            && self.output == 0
            && self.reasoning == 0
    }

    fn tokens(self) -> TurnTokens {
        TurnTokens::Codex {
            input_tokens: self.input,
            cached_input_tokens: self.cached,
            cache_write_input_tokens: self.cache_write,
            output_tokens: self.output,
            reasoning_output_tokens: self.reasoning,
            total_tokens: self.total,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Observed {
    facts: TurnFacts,
    counts: Option<Counts>,
    incomplete: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Accounting {
    previous: Option<Counts>,
    turns: HashMap<String, Observed>,
}

impl Accounting {
    pub(super) fn empty_thread(&mut self) {
        // A thread we just created has no completed requests to inherit.
        self.previous = Some(Counts { cache_write: Some(0), ..Counts::default() });
    }

    pub(super) fn started(&mut self, turn: &str) {
        self.turns.entry(turn.to_string()).or_default();
    }

    pub(super) fn usage(&mut self, turn: Option<&str>, usage: &Value) -> Option<TurnTokens> {
        let observed = turn.and_then(|turn| self.turns.get_mut(turn));
        let Some(total) = Counts::read(&usage["total"]) else {
            self.previous = None;
            if let Some(observed) = observed {
                observed.incomplete = true;
            }
            return None;
        };
        let previous = self.previous.replace(total.clone());
        if total.synthetic_full(usage["modelContextWindow"].as_u64()) {
            return None;
        }
        let Some(delta) = previous.and_then(|previous| total.difference(&previous)) else {
            if let Some(observed) = observed {
                observed.incomplete = true;
            }
            return None;
        };
        if delta.total == 0 {
            return None;
        }
        if let Some(observed) = observed {
            match &mut observed.counts {
                Some(counts) => counts.add(delta.clone()),
                None => observed.counts = Some(delta.clone()),
            }
        }
        Some(delta.tokens())
    }

    pub(super) fn rerouted(&mut self, turn: &str, model: &str) {
        if let Some(observed) = self.turns.get_mut(turn) {
            observed.facts.model(model);
        }
    }

    pub(super) fn ended(&mut self, turn: &str) -> Option<TurnFacts> {
        let mut observed = self.turns.remove(turn)?;
        if !observed.incomplete {
            observed.facts.tokens = observed.counts.map(Counts::tokens);
        }
        (!observed.facts.is_empty()).then_some(observed.facts)
    }

    pub(super) fn reloaded(&mut self) {
        self.previous = None;
    }
}

/// Recovers observations when the node ended before its host could deliver
/// a turn end. Events must be in durable acceptance order. Only a thread
/// created empty by this World may begin with a zero baseline.
pub fn recorded_turn_facts(events: &[SessionEvent], turn: &str, initially_empty: bool) -> Option<TurnFacts> {
    let mut accounting = Accounting::default();
    if initially_empty {
        accounting.empty_thread();
    }
    accounting.started(turn);
    for event in events {
        match &event.happened {
            Happened::Changed { turn: observed, change: SessionChange::Usage { usage } } => {
                let _ = accounting.usage(observed.as_deref(), usage);
            }
            Happened::Changed { turn: Some(observed), change: SessionChange::Rerouted { to, .. } }
                if observed == turn =>
            {
                accounting.rerouted(turn, to);
            }
            Happened::Changed { change: SessionChange::HistoryChanged, .. } => accounting.reloaded(),
            _ => {}
        }
    }
    accounting.ended(turn)
}
