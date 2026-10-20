//! Restarting a Linux boot service without a password (`world-upgrade.md`,
//! 换版不要人确认). Installing a system unit takes root once; that install
//! leaves a sudo rule that lets the user it serves restart that one unit and
//! nothing else, so a new release takes over by itself afterwards. Where
//! sudo or its checker is missing, no rule is written, and a renewal waits
//! for an administrator as before.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, ensure};
use fastctx_platform::atomic::{Permissions, publish};

const RULES: &str = "/etc/sudoers.d";

/// The systemctl a rule names and a restart runs: sudo matches it by path.
/// A system whose `/bin` is not `/usr/bin` has it in `/bin` alone.
pub(super) fn systemctl() -> Option<PathBuf> {
    ["/usr/bin/systemctl", "/bin/systemctl"].into_iter().map(PathBuf::from).find(|path| path.is_file())
}

fn rule_path(unit: &str) -> PathBuf {
    Path::new(RULES).join(unit)
}

/// The command a rule allows: restarting `unit` alone.
fn command(systemctl: &Path, unit: &str) -> String {
    format!("{} --system restart {unit}.service", systemctl.display())
}

/// As root: lets the account with `uid` restart `unit` from now on. Nothing
/// is written for root itself, nor where the rule cannot be checked first.
pub(super) fn allow(unit: &str, uid: u32) -> anyhow::Result<()> {
    if uid == 0 {
        return Ok(());
    }
    ensure!(
        Path::new(RULES).is_dir(),
        "{RULES} is absent; automatic service restart needs a sudoers include directory"
    );
    let systemctl = systemctl().context("systemctl is not found; cannot authorize service restart")?;
    let user = user_name(uid)?;
    let text = format!(
        "# Written by Fastctx: {user} restarts its own boot service, and nothing else, so a new release takes over.\n{user} ALL=(root) NOPASSWD: {}\n",
        command(&systemctl, unit)
    );
    // Checked where only root writes, under a name sudo passes over for the
    // dot in it, then put in place whole.
    let candidate = Path::new(RULES).join(format!(".{unit}.check"));
    publish(&candidate, text.as_bytes(), Permissions::Private)
        .with_context(|| format!("writing {}", candidate.display()))?;
    if let Err(error) = super::checked(Command::new("visudo").args(["-c", "-f"]).arg(&candidate)) {
        let _ = std::fs::remove_file(&candidate);
        return Err(error).context("validating the automatic restart sudo rule");
    }
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o440))?;
    std::fs::rename(&candidate, rule_path(unit)).with_context(|| format!("writing {}", rule_path(unit).display()))?;
    Ok(())
}

/// As root: takes the rule for `unit` away with the unit.
pub(super) fn forbid(unit: &str) -> anyhow::Result<()> {
    match std::fs::remove_file(rule_path(unit)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// Checks permission to restart and retains the policy or execution failure.
pub(super) fn may(unit: &str) -> anyhow::Result<()> {
    if fastctx_platform::elevation::is_elevated()? {
        return Ok(());
    }
    let systemctl = systemctl().context("systemctl is not found; cannot restart the boot service")?;
    super::checked(
        Command::new("sudo").args(["-n", "-l"]).args(command(&systemctl, unit).split(' ')).stdin(Stdio::null()),
    )
    .context("sudo does not authorize automatic service restart; run `fastctx renew guardian` to authorize it")?;
    Ok(())
}

/// Restarts `unit`, as root or by its rule.
pub(super) fn restart(unit: &str) -> anyhow::Result<()> {
    let service = format!("{unit}.service");
    if fastctx_platform::elevation::is_elevated()? {
        super::checked(Command::new("systemctl").args(["--system", "restart", &service]))?;
        return Ok(());
    }
    let systemctl = systemctl().context("systemctl is not where a rule names it")?;
    super::checked(Command::new("sudo").arg("-n").arg(&systemctl).args(["--system", "restart", &service]))
        .context("restarting the system service needs sudo")?;
    Ok(())
}

/// The account must be representable by one literal sudoers user name.
fn user_name(uid: u32) -> anyhow::Result<String> {
    let account = fastctx_platform::unix_account::lookup(uid)?
        .with_context(|| format!("user {uid} has no readable account record; cannot write its sudo rule"))?;
    let name = account.name.into_string().map_err(|_| anyhow::anyhow!("user {uid} has a non-UTF-8 account name"))?;
    ensure!(
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
        "user {uid} has an account name that cannot be used literally in a sudo rule"
    );
    Ok(name)
}
