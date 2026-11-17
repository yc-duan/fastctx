//! Forwarded update work keeps its original caller and resolved computer.
use super::transport::{Target, target};
use super::*;
use fastctx_types::access::{Acting, Written};

const FAMILY: Family = Family {
    name: "call/machine-updates",
    at_once: 8,
    within: Duration::from_secs(75 * 60),
    stays: Duration::from_secs(240),
    again: false,
};

#[derive(Serialize, Deserialize)]
struct Asked {
    acting: Acting,
    machine: Written,
    update: UpdateCall,
}

#[derive(Serialize, Deserialize)]
struct Answer {
    target: Target,
    result: Result<State, Failure>,
}

pub(super) fn register(node: &Arc<Node>) {
    submitted::register_resolved(node, FAMILY, |node: Arc<Node>, asked: Asked, resolved: Target| async move {
        node.settled(&asked.machine).await;
        let resolved = super::transport::resolve(&node, &asked.machine, resolved).await?;
        let actual = target(&node, &asked.machine)?;
        let changed = || {
            Failure::new(format!("{} identifies a different computer now; the update result is unknown", asked.machine))
        };
        let result = if !actual.same_computer(&resolved) {
            Err(Failure::not_run(format!(
                "{} identifies a different computer now; the accepted update was not run",
                asked.machine
            )))
        } else {
            let work = super::super::machine(&node, &asked.acting, asked.machine.clone(), asked.update);
            let result = super::transport::within(resolved.clone(), work).await;
            if target(&node, &asked.machine).is_ok_and(|now| now.same_computer(&resolved)) {
                result
            } else {
                Err(changed())
            }
        };
        Ok(Answer { target: resolved, result })
    });
}

pub(crate) async fn machine(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Written,
    update: UpdateCall,
) -> Result<State, Failure> {
    if !super::started(node) || !matches!(update, UpdateCall::Repair { .. } | UpdateCall::Kernel { .. }) {
        return super::super::machine(node, acting, machine, update).await;
    }
    let asked = Asked { acting: acting.clone(), machine, update };
    let answer: Answer = submitted::submit_resolving(node, &FAMILY, &asked, || async {
        let resolved = target(node, &asked.machine)?;
        authorize(node, &asked.acting, &asked.machine, &resolved)?;
        if matches!(resolved, Target::Here) {
            super::allowed(&asked.update).map_err(Failure::new)?;
        }
        let unit = match &resolved {
            Target::Here => "this-machine".to_string(),
            Target::Remote { identity, .. } => identity.clone(),
            Target::Pending { .. } => super::unit("machine-name", &asked.machine),
        };
        Ok((unit, resolved))
    })
    .await?;
    authorize(node, &asked.acting, &asked.machine, &answer.target)?;
    answer.result
}

fn authorize(node: &Node, acting: &Acting, machine: &Written, target: &Target) -> Result<(), Failure> {
    if matches!(target, Target::Here) {
        return Ok(());
    }
    let speakers = node
        .speakers(acting, machine, fastctx_types::access::Purpose::Hands)
        .map_err(|why| Failure::new(why.to_string()))?;
    let Target::Remote { identity, .. } = target else { return Ok(()) };
    if speakers.iter().any(|(link, _)| {
        link.view().member(&machine.name).is_some_and(|member| super::unit("machine", &member.key) == *identity)
    }) {
        Ok(())
    } else {
        Err(Failure::not_run(format!("{machine} identifies a different computer now")))
    }
}
