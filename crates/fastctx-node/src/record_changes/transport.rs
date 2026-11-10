//! Ask after accepted remote work under its original submission name.
use crate::Node;
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, MachineName,
    access::{Acting, Purpose, Written},
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    submission::Submission,
};
use std::{sync::Arc, time::Duration};

fn name() -> Result<Submission, Failure> {
    if let Some(submission) = fastctx_net::submission::current() {
        return Ok(submission);
    }
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|error| Failure::new(error.to_string()))?;
    Ok(Submission { name: bytes.iter().map(|byte| format!("{byte:02x}")).collect(), waits_ms: u32::MAX })
}

pub(crate) async fn linked(
    link: &Arc<Link>,
    to: &MachineName,
    account: &AccountName,
    purpose: Purpose,
    call: RemoteCall,
) -> Result<RemoteReply, Failure> {
    fastctx_net::submission::scope(Some(name()?), async {
        loop {
            match link
                .call(to, account, purpose, call.clone())
                .await
                .map_err(|error| Failure::new(error.to_string()))?
            {
                Err(failure) if failure.is_accepted() => tokio::time::sleep(Duration::from_secs(1)).await,
                result => return result,
            }
        }
    })
    .await
}

pub(crate) async fn on(
    node: &Arc<Node>,
    acting: &Acting,
    to: &Written,
    purpose: Purpose,
    call: RemoteCall,
) -> Result<RemoteReply, Failure> {
    fastctx_net::submission::scope(Some(name()?), async {
        loop {
            match node.call(acting, to, purpose, call.clone()).await.map_err(|error| Failure::new(error.to_string()))? {
                Err(failure) if failure.is_accepted() => tokio::time::sleep(Duration::from_secs(1)).await,
                result => return result,
            }
        }
    })
    .await
}
