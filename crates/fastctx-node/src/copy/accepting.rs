//! A copy's finite admission, separate from its potentially long execution.

use super::*;

pub(super) const ACCEPTED: &str = "copy accepted";

pub(super) async fn ask(
    node: &Node,
    acting: &Acting,
    source: &Written,
    from: String,
    to: Place,
    began: &Began,
) -> Result<CopyReport, Failure> {
    let deadline = tokio::time::Instant::now() + fastctx_types::contract::OPENED_WITHIN;
    let known = node.locate(source).map_err(|error| Failure::new(error.to_string()))?.into_iter().find_map(|link| {
        let view = link.view();
        let name = view.canonical(&source.name);
        link.versions()?.machines.get(name).map(|running| running.kernel.clone())
    });
    let version = match known {
        Some(version) => version,
        None => {
            match tokio::time::timeout_at(deadline, node.call(acting, source, Purpose::Look, RemoteCall::Facts)).await {
                Ok(Ok(Ok(RemoteReply::Facts(facts)))) => facts.version,
                Ok(Ok(Err(error))) => return Err(error),
                Ok(Err(error)) => return Err(Failure::new(error.to_string())),
                Ok(Ok(Ok(_))) => return Err(Failure::new(format!("{source} did not answer its version"))),
                Err(_) => {
                    return Err(Failure::new(format!(
                        "{source} did not answer its version before the copy's admission deadline"
                    )));
                }
            }
        }
    };
    let version = fastctx_types::update::release(&version).ok_or_else(|| {
        Failure::new(format!("cannot establish how {source} accepts copies from version {version:?}"))
    })?;
    let explicit = version >= fastctx_types::update::release("1.0.1-beta.3").expect("release version");
    let (accepted, mut heard) = tokio::sync::watch::channel(false);
    let feed = || -> Feed {
        let (accepted, began) = (accepted.clone(), began.clone());
        Box::new(move |mut stream| {
            Box::pin(async move {
                if explicit {
                    let word: String = stream.hear().await?;
                    if word != ACCEPTED {
                        return Err(format!("the copy source did not accept the request: {word}"));
                    }
                }
                // Beta.2 pairs an admitted stream before dispatching CopySend,
                // then drops that optional stream; it has no acceptance word.
                began.now();
                accepted.send_replace(true);
                stream.closed().await;
                Ok(())
            })
        })
    };
    let call = node.call_with_stream(acting, source, Purpose::Hands, RemoteCall::CopySend { from, to }, feed);
    tokio::pin!(call);
    let result = tokio::select! {
        result = &mut call => result,
        accepted = async { heard.wait_for(|accepted| *accepted).await.map(|_| ()) } => {
            accepted.map_err(|_| Failure::new("the copy's admission ended without a result"))?;
            call.await
        },
        () = tokio::time::sleep_until(deadline) => {
            return Err(Failure::new(format!(
                "{source} did not acknowledge the copy within {} seconds; whether it began is unknown",
                fastctx_types::contract::OPENED_WITHIN.as_secs()
            )));
        },
    };
    match result.map_err(|failure| asked(failure, began))? {
        Ok(RemoteReply::Copied(report)) => Ok(report),
        Ok(_) => Err(Failure::new(format!("{source} answered something other than a copy"))),
        Err(failure) => Err(failure),
    }
}
