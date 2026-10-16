//! Refusal custody is idempotent across the failure-record/held-cursor boundary.

use super::*;

pub fn record_failure(
    net: &NetDir,
    direction: &str,
    reason: &str,
    message: &Envelope,
    mut log: impl FnMut(String),
) -> io::Result<PathBuf> {
    import(net, &mut log)?;
    keep(
        net,
        &Failure {
            at: Timestamp::now(),
            direction: direction.to_string(),
            reason: reason.to_string(),
            message: message.clone(),
        },
    )
}

fn keep(net: &NetDir, failure: &Failure) -> io::Result<PathBuf> {
    let identity = serde_json::to_vec(&(&failure.direction, &failure.message)).map_err(io::Error::other)?;
    let digest: String = crate::sign::sha256(&identity).iter().map(|byte| format!("{byte:02x}")).collect();
    let path = net.failures().join(format!("letter-{digest}.json"));
    let mut bytes = serde_json::to_vec_pretty(failure).map_err(io::Error::other)?;
    bytes.push(b'\n');
    match publish_new(&path, &bytes, Permissions::Private) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let previous = std::fs::read(&path)?;
            let kept: Failure = serde_json::from_slice(&previous).map_err(io::Error::other)?;
            if kept.direction != failure.direction || kept.message != failure.message || kept.reason != failure.reason {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("conflicting failure record: {}", path.display()),
                ));
            }
            // Visibility alone does not prove the previous publication was durable.
            publish(&path, &previous, Permissions::Private)?;
        }
        Err(error) => return Err(error),
    }
    Ok(path)
}

fn import(net: &NetDir, log: &mut impl FnMut(String)) -> io::Result<()> {
    let marker = net.failures().join(".identified");
    if marker.try_exists()? {
        return Ok(());
    }
    for entry in std::fs::read_dir(net.failures())? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json")
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(|c: char| c.is_ascii_digit()))
        {
            continue;
        }
        let migrate = || -> io::Result<()> {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            let failure =
                serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            keep(net, &failure)?;
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        };
        if let Err(error) = migrate() {
            log(format!("preserved legacy failure {}: {error}", path.display()));
            if error.kind() != io::ErrorKind::InvalidData {
                return Err(error);
            }
        }
    }
    publish(&marker, b"1", Permissions::Private)
}
