//! A visitor shares the machines of its own, with whoever it names, and
//! none it was given; a machine a visitor shared is used, never passed on.
use super::*;

fn share(machine_name: &str, to: &str, shared: bool) -> AccountChange {
    AccountChange::Share { machine: machine(machine_name), to: name(to), shared, jobs: None }
}

fn said<T>(outcome: &Result<T, fastctx_client::Error>) -> String {
    outcome.as_ref().err().map(ToString::to_string).unwrap_or_default()
}

/// `owner` is the super administrator; `alice` and `bob` are visitors, and
/// `alice-two` is one of alice's machines.
pub(super) async fn exercise(owner: &World, alice: &World, bob: &World) -> Result<()> {
    let to_visitor = alice.account(None, name("alice"), share("alice-two", "bob", true)).await;
    record("a visitor shares a machine of its own with another visitor", to_visitor.is_ok(), json!(said(&to_visitor)));
    let to_member = alice.account(None, name("alice"), share("alice-two", "owner", true)).await;
    record("a visitor shares a machine of its own with a member", to_member.is_ok(), json!(said(&to_member)));
    let unknown = alice.account(None, name("alice"), share("alice-two", "nobody", true)).await;
    record(
        "a visitor's share names an account the hub has",
        said(&unknown).contains("there is no account nobody"),
        json!(said(&unknown)),
    );
    let by_visitor = bob.account(None, name("bob"), share("alice-two", "mobile", true)).await;
    record(
        "a visitor's machine is not passed on by a visitor it is shared with",
        by_visitor.is_err(),
        json!(said(&by_visitor)),
    );
    let by_member = owner.account(None, name("owner"), share("alice-two", "mobile", true)).await;
    record("nor by a member it is shared with", by_member.is_err(), json!(said(&by_member)));
    owner.account(None, name("owner"), share("owner-pc", "alice", true)).await?;
    let given = alice.account(None, name("alice"), share("owner-pc", "bob", true)).await;
    record(
        "a visitor shares none of the machines it was given",
        said(&given).contains(fastctx_types::sharing::VISITOR_SHARES_ITS_OWN),
        json!(said(&given)),
    );
    owner.account(None, name("owner"), share("owner-pc", "alice", false)).await?;
    let back = bob.account(None, name("bob"), share("alice-two", "bob", false)).await;
    record("what a visitor shared with an account is that account's to give back", back.is_ok(), json!(said(&back)));
    alice.account(None, name("alice"), share("alice-two", "owner", false)).await?;
    Ok(())
}
