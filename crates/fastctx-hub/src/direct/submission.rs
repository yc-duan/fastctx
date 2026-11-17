//! A direct-address trial is durable work on the hub that performs the trial.

use super::*;
use crate::submitted::Family;
use fastctx_net::sign::PublicKey;
use fastctx_types::{ipc::Failure, submission::Submission};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const FAMILY: Family = Family {
    name: "ask/direct",
    at_once: 4,
    within: Duration::from_secs(3 * 60),
    stays: Duration::from_secs(45),
    // A second trial can overwrite a later choice and creates another audit.
    again: false,
};

#[derive(Clone, Serialize, Deserialize)]
struct Asked {
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    machine: MachineName,
    at: Vec<SocketAddr>,
}

fn check(hub: &Hub, origin: &crate::origin::Origin, asked: &Asked) -> Result<String, String> {
    let machine = hub.store().book.directory.canonical(&asked.machine).clone();
    super::authorized(&hub.store().book.directory, origin, &asked.account, &machine, &asked.at)?;
    if !asked.at.is_empty() {
        match hub.sessions().get(&machine) {
            None => {
                return Err(format!(
                    "{machine} is offline; it must be connected to listen while the hub checks it reaches it"
                ));
            }
            Some(session) if !session.direct => {
                return Err(format!("{machine} runs an older fastctx that cannot take direct connections"));
            }
            Some(_) => {}
        }
        if hub.trials().by.contains_key(&machine) {
            return Err(format!("the hub is checking {machine} already"));
        }
    }
    Ok(format!("direct/{machine}"))
}

async fn refused(hub: &Hub, origin: crate::origin::Origin, asked: Asked, why: String) -> String {
    hub.blocking(move |hub| {
        let mut store = hub.edit_store();
        let directory = store.book.directory.clone();
        let machine = directory.canonical(&asked.machine).clone();
        let actor = Actor {
            account: Some(asked.account.clone()),
            authenticated: directory.logged_in(&origin, &asked.account),
            member: Some(origin.name),
            peer: None,
        };
        let kept = crate::machine_log::keep(
            &mut store,
            &directory,
            actor,
            Action::Direct { machine: machine.clone(), at: asked.at },
            Outcome::Refused { reason: why.clone() },
        );
        drop(store);
        hub.log.line(format_args!("direct connections to {machine} were not allowed: {why}"));
        hub.changed();
        kept.err().unwrap_or(why)
    })
    .await
}

pub(crate) fn register(hub: &Arc<Hub>) {
    crate::submitted::register(hub, FAMILY, |hub: Arc<Hub>, asked: Asked| async move {
        let origin = crate::origin::Origin { name: asked.from, key: asked.key };
        match super::change(hub, origin, asked.account, asked.machine, asked.at).await.map_err(Failure::new)? {
            Told::Done => Ok(()),
            _ => Err(Failure::new("the direct-address change answered unexpectedly")),
        }
    });
}

pub(crate) async fn change(
    hub: &Arc<Hub>,
    origin: crate::origin::Origin,
    account: AccountName,
    machine: MachineName,
    at: Vec<SocketAddr>,
    named: Option<&Submission>,
) -> Result<Told, String> {
    let asked = Asked { from: origin.name.clone(), key: origin.key, account, machine, at };
    let identity = serde_json::to_vec(&asked).map_err(|error| error.to_string())?;
    let mut refusal = None;
    let make = async || {
        let (checking, source) = (asked.clone(), origin.clone());
        let admitted = hub.blocking(move |hub| check(&hub, &source, &checking)).await;
        match admitted {
            Ok(unit) => Ok((unit, &asked)),
            Err(why) => {
                refusal = Some(why.clone());
                Err(Failure::not_run(why))
            }
        }
    };
    let came: Result<(), Failure> =
        fastctx_jobs::submitted::submit(hub.as_ref(), &FAMILY, &identity, named, make, || {}).await;
    if let Some(why) = refusal
        && came.as_ref().is_err_and(|failure| *failure == Failure::not_run(why.clone()))
    {
        return Err(refused(hub, origin, asked, why).await);
    }
    crate::submitted::told(came.map(|()| Told::Done))
}
