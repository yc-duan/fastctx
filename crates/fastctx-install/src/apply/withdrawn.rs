//! Host integrations a data-preserving uninstall restores on installation.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integration {
    pub host: Host,
    pub home: PathBuf,
    #[serde(default)]
    pub approved_limit: Option<i64>,
}

/// Lists every recorded home without recovering, locking or changing files.
pub fn integrations(places: &Places) -> Result<Vec<Integration>, Error> {
    let snapshot = Snapshot::read(places.record())?;
    let records = homes::Records::read(&snapshot, places)?;
    let mut integrations = records.integrations();
    if let Some(pending) = crate::transaction::pending_record(&places.root)? {
        for integration in homes::Records::read(&pending, places)?.integrations() {
            if !integrations.contains(&integration) {
                integrations.push(integration);
            }
        }
    }
    let paths = match std::fs::read_dir(&places.root) {
        Ok(paths) => Some(paths),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(Error::Io(error.to_string())),
    };
    for path in paths.into_iter().flatten() {
        let entry = path.map_err(io("listing plugin receipts"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("applied-claude-plugin")
            && name.ends_with(".json")
            && let Some(receipt) = load_plugin_record(&entry.path())?
            && let Some(settings) = receipt.settings_path
        {
            let home = settings.parent().ok_or_else(|| Error::Io("plugin settings have no home".into()))?;
            let integration = Integration { host: Host::Claude, home: home.to_path_buf(), approved_limit: None };
            if !integrations.contains(&integration) {
                integrations.push(integration);
            }
        }
    }
    if let Some(bytes) = Snapshot::read(places.root.join("config.toml"))?.bytes()
        && let Some(receipt) = legacy::Receipt::from_bytes(bytes)?
    {
        let home = Path::new(receipt.config_path()).parent().ok_or_else(|| Error::Io("legacy home is absent".into()))?;
        let integration = Integration { host: Host::Codex, home: home.to_path_buf(), approved_limit: None };
        if !integrations.contains(&integration) {
            integrations.push(integration);
        }
    }
    Ok(integrations)
}

fn path(root: &Path) -> PathBuf {
    root.join("uninstalled-hosts.json")
}

/// Records the complete intent before withdrawing the first integration.
pub fn remember_withdrawn(places: &Places, integrations: &[Integration]) -> Result<(), Error> {
    let mut remembered = read_withdrawn(places)?;
    let before = remembered.clone();
    for integration in integrations {
        if !remembered.contains(integration) {
            remembered.push(integration.clone());
        }
    }
    if remembered == before {
        return Ok(());
    }
    crate::disk::ensure_private_dir(&places.root).map_err(io("preparing uninstall recovery"))?;
    let bytes = serde_json::to_vec(&remembered).map_err(|error| Error::Io(error.to_string()))?;
    crate::disk::publish_private(&path(&places.root), &bytes).map_err(io("recording withdrawn integrations"))
}

pub fn read_withdrawn(places: &Places) -> Result<Vec<Integration>, Error> {
    match std::fs::read(path(&places.root)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| Error::Io(format!("withdrawn integrations: {error}")))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(Error::Io(error.to_string())),
    }
}

/// Restores only the homes withdrawn by a preserving uninstall. Successful
/// entries retire individually; an interruption can repeat the remaining ones.
pub fn restore_withdrawn(places: &Places) -> Result<Report, Error> {
    let mut remembered = read_withdrawn(places)?;
    let mut result = Report::default();
    while let Some(integration) = remembered.first().cloned() {
        let selected = places.at_home(integration.host, &integration.home)?;
        let approval = if integration.host == Host::Codex && integration.approved_limit.is_some() {
            let config = std::fs::read_to_string(selected.codex_home.join("config.toml"))
                .map_err(io("checking the original approved Codex limit"))?;
            let config: toml::Table =
                config.parse().map_err(|_| Error::Io("the Codex configuration cannot be read".into()))?;
            if config.get("tool_output_token_limit").and_then(toml::Value::as_integer) == integration.approved_limit {
                Approval::RaiseOutputLimit
            } else {
                Approval::Require
            }
        } else {
            Approval::Require
        };
        let report = apply(&selected, integration.host.only(), approval)?;
        if !report.leftovers.is_empty() || !report.notices.is_empty() {
            return Err(Error::Io(format!(
                "restoring {}: {:?}; {:?}",
                integration.home.display(),
                report.notices,
                report.leftovers
            )));
        }
        result.changed.extend(report.changed);
        result.messages.extend(report.messages);
        remembered.remove(0);
        if remembered.is_empty() {
            std::fs::remove_file(path(&places.root)).map_err(io("retiring withdrawn integrations"))?;
        } else {
            let bytes = serde_json::to_vec(&remembered).map_err(|error| Error::Io(error.to_string()))?;
            crate::disk::publish_private(&path(&places.root), &bytes).map_err(io("retiring restored integration"))?;
        }
    }
    Ok(result)
}
