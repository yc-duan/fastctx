//! Claude Code 2.1.303's built-in prices, in dollars per million tokens.
//! These weights only divide the host's reported cost; they never price a bill.

use fastctx_types::usage::Tokens;

/// Input, cache read, five-minute cache write, and output weights.
#[derive(Clone, Copy, Debug)]
pub struct Prices(pub [f64; 4]);

/// Whether the model belongs to the Fable family, including its Mythos alias.
pub fn is_fable(model: &str) -> bool {
    model.contains("fable") || model.contains("mythos")
}

/// A known model's prices, or the newest prices of its recognized family.
/// Date suffixes and context-window suffixes do not change the model's tier.
pub fn prices(model: &str, prompt: u64) -> Option<Prices> {
    let model = model.strip_suffix("[1m]").unwrap_or(model);
    let model = model
        .rsplit_once('-')
        .filter(|(_, date)| date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()))
        .map_or(model, |(model, _)| model);
    let (input, output, write, read) = match model {
        "claude-3-5-haiku" => (0.8, 4.0, 1.0, 0.08),
        "claude-haiku-4-5" => (1.0, 5.0, 1.25, 0.1),
        "claude-3-5-sonnet" | "claude-3-7-sonnet" | "claude-sonnet-4-0" | "claude-sonnet-4-5" | "claude-sonnet-4-6" => {
            (3.0, 15.0, 3.75, 0.3)
        }
        "claude-sonnet-5" => (2.0, 10.0, 2.5, 0.2),
        "claude-opus-4-0" | "claude-opus-4-1" => (15.0, 75.0, 18.75, 1.5),
        "claude-opus-4-5" | "claude-opus-4-6" | "claude-opus-4-7" | "claude-opus-4-8" | "claude-opus-5" => {
            (5.0, 25.0, 6.25, 0.5)
        }
        "claude-fable-5" | "claude-mythos-5" => (10.0, 50.0, 12.5, 1.0),
        _ if model.contains("haiku") => {
            if prompt > 100_000 {
                (0.5, 2.5, 0.625, 0.05)
            } else {
                (0.1, 0.5, 0.125, 0.01)
            }
        }
        _ if model.contains("sonnet") => (2.0, 10.0, 2.5, 0.1),
        _ if model.contains("opus") => (4.0, 20.0, 5.0, 0.2),
        _ if is_fable(model) => (10.0, 50.0, 12.5, 0.25),
        _ => return None,
    };
    Some(Prices([input, read, write, output]))
}

/// Relative cost of each kind, falling back to token counts for unknown families.
pub fn weights(model: &str, tokens: &Tokens) -> [f64; 4] {
    let prompt = tokens.input.saturating_add(tokens.cache_read).saturating_add(tokens.cache_write);
    let Prices(prices) = prices(model, prompt).unwrap_or(Prices([1.0; 4]));
    let counts = [tokens.input, tokens.cache_read, tokens.cache_write, tokens.output.saturating_add(tokens.reasoning)];
    std::array::from_fn(|i| counts[i] as f64 * prices[i])
}

/// The request's relative dollar cost at the copied list prices.
pub fn weight(model: &str, tokens: &Tokens) -> f64 {
    weights(model, tokens).iter().sum::<f64>() / 1_000_000.0
}
