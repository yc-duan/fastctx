//! Permission lifecycle uses real scope signatures and current membership snapshots.
mod fixture;
mod jobs;
use fastctx_net::directory::Level;
use fastctx_sync::{
    cursor::{Holder, Revision},
    identity::{ObjectId, ObjectKey},
    registry::REGISTRY,
    scope::{ScopeOwner, ScopeSubject, ScopeVisibility},
    store::access::{AccessContext, AccessPolicy, Decision, HoldingUse, Principal},
};
use fixture::{Result, World};
use std::sync::Arc;
fn allowed(value: Decision) -> bool {
    matches!(value, Decision::Allowed { .. })
}
fn main() -> Result<()> {
    let root =
        std::env::args_os().nth(1).map(std::path::PathBuf::from).ok_or("usage: sync_access_drill DISPOSABLE_ROOT")?;
    if fastctx_platform::disk::is_there(&root) {
        return Err("the drill root must not exist".into());
    }
    fastctx_platform::ensure_private_dir(&root)?;
    REGISTRY.validate()?;
    let mut world = World::new()?;
    let owner = ScopeOwner::Account { identity: world.identities[0].clone() };
    let audit = REGISTRY.asset("machine.audit")?.registered();
    let subject = ScopeSubject::Machine { device: world.devices[0].clone() };
    let old =
        world.descriptor(1, owner.clone(), subject.clone(), ScopeVisibility::SinceGrant { opened_acl: Revision(1) }, 2);
    let new =
        world.descriptor(2, owner.clone(), subject.clone(), ScopeVisibility::SinceGrant { opened_acl: Revision(2) }, 2);
    let reader = Principal::Account { account: world.identities[1].id.clone(), device: world.devices[1].clone() };
    for (descriptor, expected) in [(old.clone(), false), (new.clone(), true)] {
        let object = ObjectKey { scope: descriptor.scope.clone(), object: ObjectId::from_bytes([10; 32]) };
        let context = AccessContext {
            principal: reader.clone(),
            scope: descriptor.scope.clone(),
            observed_acl: descriptor.acl_revision,
        };
        let policy =
            world.policy(world.verify(descriptor)?, Holder::Device { device: world.devices[1].clone() }, Some(2))?;
        assert_eq!(allowed(policy.can_read_object(&context, audit, &object)), expected);
        assert_eq!(allowed(policy.can_keep(&context, audit, &object, &HoldingUse::Replica)), expected);
    }
    let current = world.descriptor(3, owner.clone(), subject.clone(), ScopeVisibility::Current, 2);
    let context = AccessContext { principal: reader.clone(), scope: current.scope.clone(), observed_acl: Revision(2) };
    let policy = world.policy(world.verify(current)?, Holder::Hub { hub: world.hub.clone() }, Some(2))?;
    assert!(!allowed(policy.can_read(&context, audit)), "audit cannot use an unsegmented machine scope");
    world.directory.shares.clear();
    let mut revoked = new.clone();
    revoked.acl_revision = Revision(3);
    revoked.revision.revision = Revision(3);
    let context = AccessContext { principal: reader.clone(), scope: revoked.scope.clone(), observed_acl: Revision(3) };
    let policy = world.policy(world.verify(revoked)?, Holder::Hub { hub: world.hub.clone() }, None)?;
    assert!(!allowed(policy.can_read(&context, audit)));
    world.directory.shares.push(Arc::new(fastctx_net::directory::Share {
        machine: "machine-0".parse()?,
        account: world.identities[1].name.clone(),
        jobs: None,
    }));
    let mut regranted = new;
    regranted.acl_revision = Revision(4);
    regranted.revision.revision = Revision(4);
    let context =
        AccessContext { principal: reader.clone(), scope: regranted.scope.clone(), observed_acl: Revision(4) };
    let policy = world.policy(world.verify(regranted)?, Holder::Hub { hub: world.hub.clone() }, Some(4))?;
    assert!(!allowed(policy.can_read(&context, audit)), "regrant cannot inherit an older segment");

    let admin = world.descriptor(4, ScopeOwner::Hub, ScopeSubject::Administrators, ScopeVisibility::Current, 1);
    let context = AccessContext { principal: reader.clone(), scope: admin.scope.clone(), observed_acl: Revision(1) };
    let policy = world.policy(world.verify(admin.clone())?, Holder::Hub { hub: world.hub.clone() }, Some(4))?;
    assert!(!allowed(policy.can_read(&context, audit)));
    Arc::make_mut(&mut world.directory.accounts[0]).level = Level::Member;
    Arc::make_mut(&mut world.directory.accounts[1]).level = Level::Super;
    let policy = world.policy(world.verify(admin)?, Holder::Device { device: world.devices[1].clone() }, Some(4))?;
    assert!(allowed(policy.can_read(&context, audit)));
    let mut agent = context.clone();
    agent.principal = Principal::Agent {
        account: world.identities[1].id.clone(),
        device: world.devices[1].clone(),
        conversation: ObjectKey { scope: agent.scope.clone(), object: ObjectId::from_bytes([9; 32]) },
    };
    assert!(!allowed(policy.can_read(&agent, audit)), "a logged-in device does not turn an agent into a user");

    let conversation = world.descriptor(
        5,
        owner,
        ScopeSubject::Conversation { conversation: ObjectId::from_bytes([5; 32]), shared_on: None, born_seal: None },
        ScopeVisibility::Current,
        1,
    );
    let object = ObjectKey { scope: conversation.scope.clone(), object: ObjectId::from_bytes([6; 32]) };
    let mut context = AccessContext {
        principal: Principal::Account { account: world.identities[0].id.clone(), device: world.devices[0].clone() },
        scope: conversation.scope.clone(),
        observed_acl: Revision(1),
    };
    let policy = world.policy(world.verify(conversation)?, Holder::Hub { hub: world.hub.clone() }, Some(4))?;
    let timer = REGISTRY.asset("conversation.timer")?.registered();
    assert!(allowed(policy.can_read_object(&context, timer, &object)));
    context.principal = Principal::Agent {
        account: world.identities[0].id.clone(),
        device: world.devices[0].clone(),
        conversation: object.clone(),
    };
    assert!(!allowed(policy.can_read_object(&context, timer, &object)));
    context.principal = Principal::LocalService;
    assert!(!allowed(policy.can_read_object(&context, timer, &object)));
    jobs::run()?;
    println!(
        "scope lifecycle: real signatures, grant segments, revocation/regrant, administrator transfer, user-only reads and page job custody passed"
    );
    Ok(())
}
