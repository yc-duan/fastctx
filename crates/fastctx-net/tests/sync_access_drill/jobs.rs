//! A consenting page user, project owner and execution machine can all differ.
use super::*;
use fastctx_net::sync_access::{SourceCustody, SourcePhase};
use fastctx_sync::{
    identity::{OperationId, ProducerEpoch},
    store::access::{JobBinding, JobSourceOwner},
};

pub(super) fn run() -> Result<()> {
    let mut world = World::new()?;
    Arc::make_mut(&mut world.directory.members[2]).managed = true;
    let project = world.descriptor(
        70,
        ScopeOwner::Account { identity: world.identities[0].clone() },
        ScopeSubject::Project {
            project: ObjectId::from_bytes([70; 32]),
            town: fastctx_types::access::TownId::from_bytes([70; 8]),
        },
        ScopeVisibility::Current,
        1,
    );
    let private = world.descriptor(
        71,
        ScopeOwner::Account { identity: world.identities[1].clone() },
        ScopeSubject::Account,
        ScopeVisibility::Current,
        1,
    );
    let project_key = ObjectKey { scope: project.scope.clone(), object: ObjectId::from_bytes([70; 32]) };
    let page = ObjectKey { scope: project.scope.clone(), object: ObjectId::from_bytes([72; 32]) };
    let kind = REGISTRY.asset("job.output")?.registered();
    for descriptor in [project, private] {
        let object = ObjectKey { scope: descriptor.scope.clone(), object: ObjectId::from_bytes([73; 32]) };
        let job = JobBinding {
            job: ObjectKey { scope: descriptor.scope.clone(), object: ObjectId::from_bytes([74; 32]) },
            epoch: ProducerEpoch::from_bytes([75; 32]),
            owner: JobSourceOwner::Page { project: project_key.clone(), page: page.clone() },
            account: world.identities[1].id.clone(),
            executor: world.devices[2].clone(),
            dispatcher: world.devices[1].clone(),
            dispatch: OperationId::from_bytes([76; 32]),
        };
        let purpose = HoldingUse::JobSource { job: job.clone() };
        let context = AccessContext {
            principal: Principal::LocalService,
            scope: descriptor.scope.clone(),
            observed_acl: Revision(1),
        };
        let holder = Holder::Device { device: world.devices[2].clone() };
        let source = SourceCustody {
            object: object.clone(),
            kind: "job.output".into(),
            binding: purpose.clone(),
            phase: SourcePhase::Active,
        };
        let make =
            |source| world.policy_with_source(world.verify(descriptor.clone())?, holder.clone(), Some(2), source);
        let policy = make(None)?;
        assert!(!allowed(policy.can_keep(&context, kind, &object, &purpose)), "a claimed Page owner is not provenance");
        let policy = make(Some(source.clone()))?;
        assert!(allowed(policy.can_keep(&context, kind, &object, &purpose)));
        assert!(
            !allowed(policy.can_keep(&context, kind, &object, &HoldingUse::Replica)),
            "source custody is not an ordinary replica grant"
        );
        let other = ObjectKey { object: ObjectId::from_bytes([77; 32]), ..object.clone() };
        assert!(!allowed(policy.can_keep(&context, kind, &other, &purpose)));
        let mut changed = job;
        changed.epoch = ProducerEpoch::from_bytes([78; 32]);
        assert!(!allowed(policy.can_keep(&context, kind, &object, &HoldingUse::JobSource { job: changed })));
        let policy = make(Some(SourceCustody { phase: SourcePhase::AwaitingHub, ..source.clone() }))?;
        assert!(allowed(policy.can_keep(&context, kind, &object, &purpose)), "sealed output survives until hub ACK");
        let policy = make(Some(SourceCustody { phase: SourcePhase::Released, ..source }))?;
        assert!(!allowed(policy.can_keep(&context, kind, &object, &purpose)));
    }
    Ok(())
}
