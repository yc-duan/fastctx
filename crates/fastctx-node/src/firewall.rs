//! This machine's own firewall, when it takes direct connections
//! (`world-network.md`, 直连): on Windows the World lets the ports in for its
//! own program; elsewhere it only looks at which firewall is on, since the
//! World does not change another system's firewall, nor any cloud
//! provider's security group. What it finds tells the hub's check which
//! layer kept the hub out.

use std::collections::BTreeSet;

use fastctx_net::ids::HubId;
use fastctx_net::wire::Firewall;

/// Lets other computers reach this machine at exactly the TCP `ports`, for
/// the hub `hub`, as far as the World does that here, and says what it
/// knows of the firewall. With no ports, what was let in for that hub is
/// closed again.
pub(crate) fn open(hub: &HubId, ports: &BTreeSet<u16>) -> Result<Firewall, String> {
    if cfg!(windows) { windows::open(hub, ports) } else { Ok(look()) }
}

/// Which firewall is on, on a system whose firewall the World leaves alone.
fn look() -> Firewall {
    if !cfg!(target_os = "linux") {
        return Firewall::Unknown { why: "the World does not look at this system's firewall".to_string() };
    }
    // ufw says whether it is on in a file anyone may read; firewalld and
    // nftables are services whose state anyone may ask. Rules loaded
    // without either only root can list.
    let ufw = std::fs::read_to_string("/etc/ufw/ufw.conf")
        .is_ok_and(|conf| conf.lines().any(|line| line.trim().eq_ignore_ascii_case("ENABLED=yes")));
    if ufw {
        return Firewall::Other { name: "ufw".to_string() };
    }
    for service in ["firewalld", "nftables"] {
        let mut asking = std::process::Command::new("systemctl");
        asking.args(["is-active", "--quiet", service]).stdin(std::process::Stdio::null());
        let active =
            fastctx_platform::bounded::output(asking, "systemctl is-active", std::time::Duration::from_secs(10))
                .is_ok_and(|answer| answer.status.success());
        if active {
            return Firewall::Other { name: service.to_string() };
        }
    }
    Firewall::NoneFound
}

#[cfg(windows)]
mod windows {
    use std::collections::BTreeSet;

    use fastctx_install::direct_firewall;
    use fastctx_net::ids::HubId;
    use fastctx_net::wire::Firewall;

    /// Allows inbound TCP at `ports` to this very program, replacing the
    /// rule this World had for the hub, under the name nodes gave it before
    /// as well; with no ports, only removes it. Changing Windows Firewall
    /// takes an administrator's rights, which a node running as a user
    /// without them does not have: it then says so and leaves the firewall
    /// as it is.
    pub fn open(hub: &HubId, ports: &BTreeSet<u16>) -> Result<Firewall, String> {
        let listed = ports.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        let tcp = if ports.len() == 1 { format!("TCP port {listed}") } else { format!("TCP ports {listed}") };
        let elevated = fastctx_platform::elevation::is_elevated()
            .map_err(|error| format!("asking Windows for this node's rights: {error}"))?;
        if !elevated {
            let cannot = if ports.is_empty() {
                "remove its Windows Firewall rule".to_string()
            } else {
                format!("let {tcp} in through Windows Firewall")
            };
            return Ok(Firewall::Unknown {
                why: format!("this node does not run as an administrator, so the World cannot {cannot}"),
            });
        }
        let sid = fastctx_platform::current_user_sid().map_err(|error| format!("finding this user: {error}"))?;
        let rule = direct_firewall::rule(&sid, hub.as_str());
        let earlier =
            std::env::var("USERNAME").ok().map(|account| direct_firewall::earlier_rule(&account, hub.as_str()));
        for name in [Some(&rule), earlier.as_ref()].into_iter().flatten() {
            direct_firewall::remove(name)
                .map_err(|error| format!("running netsh: {error}"))?
                .map_err(|code| format!("removing the earlier Windows Firewall rule failed (netsh exit {code:?})"))?;
        }
        if ports.is_empty() {
            return Ok(Firewall::Opened);
        }
        let program = std::env::current_exe().map_err(|error| format!("finding this program: {error}"))?;
        match direct_firewall::allow(&rule, &program, ports).map_err(|error| format!("running netsh: {error}"))? {
            Ok(()) => Ok(Firewall::Opened),
            Err(code) => Ok(Firewall::Unknown {
                why: format!("Windows Firewall did not take the rule letting {tcp} in (netsh exit {code:?})"),
            }),
        }
    }
}

#[cfg(not(windows))]
mod windows {
    use std::collections::BTreeSet;

    use fastctx_net::ids::HubId;

    pub fn open(_hub: &HubId, _ports: &BTreeSet<u16>) -> Result<fastctx_net::wire::Firewall, String> {
        unreachable!("only Windows opens its firewall here")
    }
}
