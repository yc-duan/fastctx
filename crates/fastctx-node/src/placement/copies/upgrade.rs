//! Complete the old durable journal once before the new replica layer publishes.
use super::*;

pub(crate) async fn take(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    account: &fastctx_types::AccountName,
    owner: &Address,
) -> Result<(), Failure> {
    let slots: Vec<_> = node
        .copies
        .slots
        .lock()
        .expect("never poisoned")
        .values()
        .filter(|slot| {
            slot.account.hub == link.membership().id
                && &slot.account.account == account
                && slot.owner.resident == owner.resident
        })
        .cloned()
        .collect();
    for slot in slots {
        let mut state = slot.state.lock().await;
        if state.upgraded {
            continue;
        }
        let view = crate::authorization::current_view(link)?;
        if view.account(&state.account.account).is_none_or(|account| account.opened != state.opened) {
            return Err(failure("the legacy plan belongs to an earlier account incarnation"));
        }
        if state.retiring {
            retire::finish(node, &slot, &mut state, link).await?;
        }
        journal::settle(node, &slot, &mut state, link).await?;
        let page = link.plan_read(&state.account.account, state.owner.resident.clone()).await.map_err(failure)?;
        if let Some(text) = read(&slot.path)? {
            let dirty = if state.initial { !had(link, &state, &page, &text).await? } else { text != state.text };
            if dirty {
                state.submission = Some(journal::Submission {
                    id: fastctx_net::plan_merge::submission(&state.owner, state.base, &text),
                    base: state.base,
                    text,
                });
                keep(node, &slot.key, &state)?;
                journal::settle(node, &slot, &mut state, link).await?;
            }
        }
        state.upgraded = true;
        keep(node, &slot.key, &state)?;
    }
    Ok(())
}
