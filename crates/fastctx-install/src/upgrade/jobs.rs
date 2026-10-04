use std::path::Path;

use serde::Deserialize;

use super::Plan;

#[derive(Deserialize)]
struct Metadata {
    schema_version: u32,
    supervisor: Supervisor,
}

#[derive(Deserialize)]
struct Supervisor {
    pid: u32,
    started: String,
}

pub(super) fn inspect(root: &Path, plan: &mut Plan) {
    for directory in super::entries(&root.join("jobs"), plan) {
        if std::fs::symlink_metadata(&directory).is_ok_and(|metadata| crate::disk::is_link(&metadata)) {
            if plan.quiet(&directory, false) && plan.unused(&directory, true) {
                plan.archive(&directory, "jobs", "job link retained as a link; its target was not traversed");
            }
            continue;
        }
        let valid_name = directory.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
            name.len() == 8
                && name.starts_with("j-")
                && name[2..].bytes().all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
        });
        if !valid_name {
            if plan.quiet(&directory, false) && plan.unused(&directory, true) {
                plan.archive(&directory, "jobs", "unrecognized job entry retained without interpreting its contents");
            }
            continue;
        }
        let Some(snapshot) = plan.read(&directory.join("meta.json")) else { continue };
        let Some(bytes) = snapshot.bytes() else {
            if plan.quiet(&directory, false) && plan.unused(&directory, true) {
                plan.archive(&directory, "jobs", "abandoned job reservation; no legacy process remains");
            }
            continue;
        };
        let metadata = match serde_json::from_slice::<Metadata>(bytes) {
            Ok(metadata) if (1..=3).contains(&metadata.schema_version) && metadata.supervisor.pid != 0 => metadata,
            _ => {
                if plan.quiet(&directory, false) && plan.unused(&directory, true) {
                    plan.archive(
                        &directory,
                        "jobs",
                        "modified or unrecognized job metadata and output retained unchanged",
                    );
                }
                continue;
            }
        };
        match alive(&metadata.supervisor) {
            Ok(true) => {
                plan.active(
                    &directory,
                    format!(
                        "user job supervisor PID {} is still running; no process was stopped",
                        metadata.supervisor.pid
                    ),
                );
            }
            Err(error) => {
                plan.active(&directory, format!("cannot determine supervisor lifetime: {error}; preserved"));
            }
            Ok(false) => {
                if plan.quiet(&directory, false) && plan.unused(&directory, true) {
                    plan.archive(
                        &directory,
                        "jobs",
                        "ended job: records, output and user additions retained unchanged",
                    );
                }
            }
        }
    }
    plan.empty_after(&root.join("jobs"));
}

fn alive(supervisor: &Supervisor) -> std::io::Result<bool> {
    Ok(super::processes::legacy_token(supervisor.pid)?.as_deref() == Some(supervisor.started.as_str()))
}
