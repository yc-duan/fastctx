//! The inbound rule a node keeps on Windows while other machines may connect
//! to it directly for a hub (`world-network.md`, 直连): one per user and hub,
//! letting the World's own program in at the ports it listens on. The node
//! makes, replaces and removes it as the hub's word changes; removing
//! fastctx from the computer removes it too, the node being stopped by then.
//! Changing Windows Firewall takes an administrator's rights. `netsh`
//! answers in the system's language, so only its exit status is read.

/// The rule's name for the user `sid` and the hub `hub`.
pub fn rule(sid: &str, hub: &str) -> String {
    format!("fastctx direct connections ({sid}, {hub})")
}

/// The name nodes gave the rule before, by the user's account name.
pub fn earlier_rule(account: &str, hub: &str) -> String {
    format!("fastctx direct connections ({account}, {hub})")
}

#[cfg(windows)]
mod netsh {
    use std::collections::BTreeSet;
    use std::io;
    use std::path::Path;
    use std::process::Command;

    /// Runs `netsh advfirewall firewall` with `args`: whether it succeeded,
    /// else its exit code.
    fn netsh(args: &[&str]) -> io::Result<Result<(), Option<i32>>> {
        let output = Command::new("netsh")
            .args(["advfirewall", "firewall"])
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()?;
        Ok(if output.status.success() { Ok(()) } else { Err(output.status.code()) })
    }

    /// Whether a rule named `name` is there.
    pub fn present(name: &str) -> io::Result<bool> {
        Ok(netsh(&["show", "rule", &format!("name={name}")])?.is_ok())
    }

    /// Removes the rule named `name` if it is there; else netsh's exit code.
    pub fn remove(name: &str) -> io::Result<Result<(), Option<i32>>> {
        if !present(name)? {
            return Ok(Ok(()));
        }
        netsh(&["delete", "rule", &format!("name={name}")])
    }

    /// Lets inbound TCP at `ports` in to `program` under the rule `name`;
    /// else netsh's exit code.
    pub fn allow(name: &str, program: &Path, ports: &BTreeSet<u16>) -> io::Result<Result<(), Option<i32>>> {
        let listed = ports.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        netsh(&[
            "add",
            "rule",
            &format!("name={name}"),
            "dir=in",
            "action=allow",
            "protocol=TCP",
            &format!("localport={listed}"),
            &format!("program={}", program.display()),
            "enable=yes",
            "profile=any",
        ])
    }
}

#[cfg(windows)]
pub use netsh::{allow, present, remove};
