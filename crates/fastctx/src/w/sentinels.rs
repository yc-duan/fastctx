//! Sentinel rows share the activity view, with failures reported separately.
use fastctx_client::Client;
use fastctx_types::{
    Call, Reply, SentinelId,
    sentinel_control::{Reply as SentinelReply, Request, Summary},
    sentinel_words,
};

pub(super) fn named(text: &str) -> bool {
    text.split_once('@').map_or(text, |(id, _)| id).parse::<SentinelId>().is_ok()
}
pub(super) fn list(
    runtime: &tokio::runtime::Runtime,
    client: &mut Client,
) -> anyhow::Result<(Vec<Summary>, Vec<String>)> {
    if !client.sentinels {
        return Ok((Vec::new(), Vec::new()));
    }
    match runtime.block_on(client.call(Call::Sentinel {
        hub: None,
        account: None,
        request: Request::List { owner: None },
    })) {
        Ok(Reply::Sentinel(reply)) => match *reply {
            SentinelReply::Listed { sentinels, unavailable } => Ok((sentinels, unavailable)),
            _ => anyhow::bail!("the node did not list sentinels"),
        },
        Err(fastctx_client::CallError::Failed(error)) => Ok((Vec::new(), vec![error.message])),
        Err(error) => Err(error.into()),
        _ => anyhow::bail!("the node did not list sentinels"),
    }
}
pub(super) fn detail(names: &[String], sentinels: &[Summary], errors: &[String]) -> anyhow::Result<String> {
    let mut text = String::new();
    for name in names {
        let (id, home) = name.split_once('@').map_or((name.as_str(), None), |(id, home)| (id, Some(home)));
        let home = home.map(fastctx_types::access::Written::parse).transpose()?;
        let found: Vec<_> = sentinels
            .iter()
            .filter(|s| {
                s.definition.id.as_str() == id
                    && home.as_ref().is_none_or(|home| {
                        home.name == s.definition.home && home.hub.as_ref().is_none_or(|hub| *hub == s.definition.hub)
                    })
            })
            .collect();
        if found.is_empty() {
            text.push_str(&format!("{name}: no active sentinel found in the accounts that answered\n"));
        }
        for sentinel in found {
            text.push_str(&sentinel_words::created(sentinel));
        }
    }
    text.push_str(&unavailable(errors));
    Ok(text)
}
pub(super) fn unavailable(errors: &[String]) -> String {
    errors.iter().map(|e| format!("sentinels unavailable: {e}\n")).collect()
}
