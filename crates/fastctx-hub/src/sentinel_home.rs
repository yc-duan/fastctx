//! Choosing a machine shared with an account as its sentinel home is the
//! account's word that the machine may act for it, as using an agent there is
//! (`world-network.md`, 别人机器上的智能体什么时候能访问你的机器): the sentinel
//! lives there and looks at the account's other machines from there.
use crate::Hub;
use fastctx_types::{AccountName, MachineName, sentinel_settings::Settings};
use jiff::Timestamp;
use serde_json::Value;

/// Notes that the home of the settings `account` wrote acts for it from now on,
/// when that machine is the account's own or shared with it, and is no managed one.
pub(crate) fn chosen(hub: &Hub, by: &MachineName, account: &AccountName, body: &Value) {
    let Ok(Settings { home: Some(home), .. }) = serde_json::from_value::<Settings>(body.clone()) else { return };
    let noted = {
        let mut store = hub.store();
        let directory = &store.book.directory;
        let Some(member) = directory.member(&home) else { return };
        let machine = member.name.clone();
        let given = member.home.as_ref() == Some(account) || directory.shared(account, &machine);
        let rights = directory.rights(account, &machine);
        if member.managed || !given || !(rights.hands && rights.agents) || directory.may_act_for(account, &machine) {
            return;
        }
        let before = store.book.clone();
        store.book.directory.acting.push(std::sync::Arc::new(fastctx_net::directory::ActsFor {
            machine: machine.clone(),
            account: account.clone(),
            since: Timestamp::now(),
        }));
        let actor = fastctx_types::machine_log::Actor {
            account: Some(account.clone()),
            authenticated: true,
            member: Some(by.clone()),
            peer: None,
        };
        let action = fastctx_types::machine_log::Action::AllowSentinelHome { machine: machine.clone() };
        if let Err(reason) = crate::machine_log::keep(
            &mut store,
            &before.directory,
            actor,
            action,
            fastctx_types::machine_log::Outcome::Done,
        ) {
            store.book = before;
            hub.log.line(format_args!("{machine} could not act for {account} as its sentinel home: {reason}"));
            return;
        }
        machine
    };
    hub.log.line(format_args!("{noted} acts for {account} from now on: it is the account's sentinel home"));
    hub.changed();
}
