//! Where a protocol's requests go. Providers publish a base address
//! (`https://api.deepseek.com`, `https://api.anthropic.com`); the protocol's
//! own path is added to it, and a full endpoint is used as it is given. A
//! query the address carries (`?api-version=…`) stays at its end.

use fastctx_types::agent::ModelProtocol;

pub(super) fn of(protocol: ModelProtocol, given: &str) -> String {
    let given = given.trim();
    let (address, query) = match given.split_once('?') {
        Some((address, query)) => (address, format!("?{query}")),
        None => (given, String::new()),
    };
    let address = address.trim_end_matches('/');
    let endpoint = match protocol {
        ModelProtocol::OpenAi if address.ends_with("/chat/completions") => address.into(),
        ModelProtocol::OpenAi => format!("{address}/chat/completions"),
        ModelProtocol::Anthropic if address.ends_with("/messages") => address.into(),
        ModelProtocol::Anthropic if address.ends_with("/v1") => format!("{address}/messages"),
        ModelProtocol::Anthropic => format!("{address}/v1/messages"),
    };
    format!("{endpoint}{query}")
}
