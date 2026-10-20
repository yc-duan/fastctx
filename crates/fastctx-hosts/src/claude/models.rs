//! The Claude models the World offers, named by their full model ids.
//!
//! Claude Code's own picker shows a list that Anthropic's service sends per
//! account, and a plugin cannot read it; its aliases (`opus` and the like)
//! are resolved when a session starts but not when a request's model is
//! replaced, where an alias is refused and quietly falls back to an older
//! model (`world-gui.md`, 对话). So the World carries this table, updated with
//! each World release, and only ever passes a full id: `--model` when a
//! conversation starts, and the request's model when the user switches.
//!
//! The rows, their order and their efforts were read from Claude Code 2.1.296;
//! the order is that of its own picker. A login may lack a family (Fable on some plans): the mod
//! reports which aliases the login offers, and a family it does not offer is
//! left out.

use fastctx_types::agents::{AgentModel, ReasoningEffort};
use fastctx_types::session::control::ModelOffer;

const FIVE: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const NO_XHIGH: &[&str] = &["low", "medium", "high", "max"];
const NONE: &[&str] = &[];

/// A model's context window when Claude Code uses it without a `[1m]` suffix.
const MILLION: u64 = 1_000_000;
const TWO_HUNDRED_THOUSAND: u64 = 200_000;

/// One row of the table.
pub struct ClaudeModel {
    pub id: &'static str,
    pub name: &'static str,
    /// The alias Claude Code resolves to this model's family.
    pub family: &'static str,
    /// The efforts it takes; Claude Code quietly lowers an effort the model
    /// does not take to `high`, so only these are offered.
    pub efforts: &'static [&'static str],
    /// Claude Code's own default effort for it, which the account's settings
    /// may change; the World always names an effort.
    pub default_effort: Option<&'static str>,
    pub context_window: u64,
}

pub const MODELS: &[ClaudeModel] = &[
    row("claude-opus-5-5", "Opus 5.5", "opus", FIVE, Some("medium"), MILLION),
    row("claude-fable-5-1", "Fable 5.1", "fable", FIVE, Some("high"), MILLION),
    row("claude-sonnet-5-5", "Sonnet 5.5", "sonnet", FIVE, Some("medium"), MILLION),
    row("claude-haiku-4-5-20251001", "Haiku 4.5", "haiku", NONE, None, TWO_HUNDRED_THOUSAND),
    row("claude-sonnet-5", "Sonnet 5", "sonnet", FIVE, Some("high"), MILLION),
    row("claude-opus-5", "Opus 5", "opus", FIVE, Some("high"), MILLION),
    row("claude-fable-5", "Fable 5", "fable", FIVE, Some("high"), MILLION),
    row("claude-opus-4-8", "Opus 4.8", "opus", FIVE, Some("high"), MILLION),
    row("claude-opus-4-7", "Opus 4.7", "opus", FIVE, Some("xhigh"), MILLION),
    row("claude-opus-4-6", "Opus 4.6", "opus", NO_XHIGH, Some("high"), TWO_HUNDRED_THOUSAND),
    row("claude-sonnet-4-6", "Sonnet 4.6", "sonnet", NO_XHIGH, Some("high"), TWO_HUNDRED_THOUSAND),
    // Claude Code 2.1.293 and later.
    row("claude-haiku-5-5", "Haiku 5.5", "haiku", FIVE, Some("medium"), MILLION),
];

const fn row(
    id: &'static str,
    name: &'static str,
    family: &'static str,
    efforts: &'static [&'static str],
    default_effort: Option<&'static str>,
    context_window: u64,
) -> ClaudeModel {
    ClaudeModel { id, name, family, efforts, default_effort, context_window }
}

/// The rows a login offers. `aliases` are the model aliases its Claude Code
/// listed in a session (`None` before any session has run there, when every
/// family is offered).
pub fn offered(aliases: Option<&[String]>) -> impl Iterator<Item = &'static ClaudeModel> {
    let families: Option<Vec<String>> = aliases.map(|aliases| aliases.iter().map(|alias| family_of(alias)).collect());
    MODELS.iter().filter(move |model| match &families {
        Some(families) => families.iter().any(|family| family == model.family),
        None => true,
    })
}

/// The family an alias Claude Code lists belongs to: `sonnet[1m]` and
/// `opusplan` name the families they start with.
fn family_of(alias: &str) -> String {
    let alias = alias.to_ascii_lowercase();
    ["opus", "fable", "sonnet", "haiku"]
        .into_iter()
        .find(|family| alias.starts_with(family))
        .map_or(alias.clone(), str::to_owned)
}

/// The row of a full model id.
pub fn find(id: &str) -> Option<&'static ClaudeModel> {
    MODELS.iter().find(|model| model.id == id)
}

/// The table as the agent picker lists it; the first offered row is the
/// default when the user has chosen none.
pub fn agent_models(aliases: Option<&[String]>) -> Vec<AgentModel> {
    offered(aliases)
        .enumerate()
        .map(|(at, model)| AgentModel {
            id: model.id.to_string(),
            model: model.id.to_string(),
            display_name: model.name.to_string(),
            description: String::new(),
            is_default: at == 0,
            default_reasoning_effort: model.default_effort.unwrap_or_default().to_string(),
            reasoning_efforts: model
                .efforts
                .iter()
                .map(|effort| ReasoningEffort { effort: (*effort).to_string(), description: String::new() })
                .collect(),
            context_window: Some(model.context_window),
            max_context_window: Some(model.context_window),
        })
        .collect()
}

/// The table as a conversation's model panel lists it.
pub fn model_offers(aliases: Option<&[String]>) -> Vec<ModelOffer> {
    offered(aliases)
        .enumerate()
        .map(|(at, model)| ModelOffer {
            id: model.id.to_string(),
            name: Some(model.name.to_string()),
            description: None,
            efforts: model.efforts.iter().map(|effort| (*effort).to_string()).collect(),
            default_effort: model.default_effort.map(str::to_owned),
            default: at == 0,
            service_tiers: Vec::new(),
            default_service_tier: None,
            context_window: Some(model.context_window),
        })
        .collect()
}
