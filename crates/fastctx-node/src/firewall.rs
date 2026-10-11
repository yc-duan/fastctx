//! This machine's own firewall, when it takes direct connections
//! (`world-network.md`, 直连): on Windows the World lets the port in for its
//! own program; elsewhere it only looks at which firewall is on, since the
//! World does not change another system's firewall, nor any cloud
//! provider's security group. What it finds tells the hub's check which
//! layer kept the hub out.

use fastctx_net::wire::Firewall;

/// Lets other computers reach this machine at TCP `port`, as far as the
/// World does that here, and says what it knows of the firewall.
pub(crate) fn open(port: u16) -> Result<Firewall, String> {
    if cfg!(windows) { windows::open(port) } else { Ok(look()) }
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
        let active = std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", service])
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if active {
            return Firewall::Other { name: service.to_string() };
        }
    }
    Firewall::NoneFound
}

#[cfg(windows)]
mod windows {
    use std::process::Command;

    use fastctx_net::wire::Firewall;

    /// The rule's name: one per user's World, so two users' machines on one
    /// computer keep their own.
    fn rule() -> String {
        let user = std::env::var("USERNAME").unwrap_or_default();
        format!("name=fastctx direct connections ({user})")
    }

    /// Allows inbound TCP at `port` to this very program, replacing any
    /// rule this World had for another port. `netsh` answers in the
    /// system's language, so only its exit status is read; it needs an
    /// administrator's rights, which a node running as a user without them
    /// does not have.
    pub fn open(port: u16) -> Result<Firewall, String> {
        let program = std::env::current_exe().map_err(|error| format!("finding this program: {error}"))?;
        let _ = Command::new("netsh").args(["advfirewall", "firewall", "delete", "rule", &rule()]).output();
        let output = Command::new("netsh")
            .args(["advfirewall", "firewall", "add", "rule", &rule(), "dir=in", "action=allow", "protocol=TCP"])
            .arg(format!("localport={port}"))
            .arg(format!("program={}", program.display()))
            .args(["enable=yes", "profile=any"])
            .output()
            .map_err(|error| format!("running netsh: {error}"))?;
        if output.status.success() {
            return Ok(Firewall::Opened);
        }
        let said = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Firewall::Unknown {
            why: format!(
                "the World could not let TCP port {port} in through Windows Firewall, which takes an administrator (netsh exit {:?}: {said})",
                output.status.code()
            ),
        })
    }
}

#[cfg(not(windows))]
mod windows {
    pub fn open(_port: u16) -> Result<fastctx_net::wire::Firewall, String> {
        unreachable!("only Windows opens its firewall here")
    }
}
