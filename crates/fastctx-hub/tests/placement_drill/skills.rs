//! Exercise the materialized folder, not just the account JSON protocol.
use super::*;
use fastctx_types::{
    Address, Call, ResidentId,
    skill_files::{Change, File, Reply, Request},
};
fn folder(root: &Path) -> Result<PathBuf> {
    for entry in std::fs::read_dir(root.join("world/skill-copies"))? {
        let path = entry?.path().join("kit-1");
        if path.is_dir() {
            return Ok(path);
        }
    }
    anyhow::bail!("the skill was not materialized")
}
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

pub(super) async fn exercise(
    world: &World,
    host: &Path,
    computer: &Path,
    id: &ResidentId,
    read_script: &str,
) -> Result<()> {
    let original = folder(computer)?;
    std::fs::remove_file(original.join("references/note.txt"))?;
    world.talk(id.clone(), Some("host".parse()?), read_script.into()).await?;
    anyhow::ensure!(idle(host, id.as_str()).await, "repair turn did not finish");
    record(
        "a missing auxiliary file is repaired before the folder is returned",
        file_is(&original.join("references/note.txt"), "ALICE_REFERENCE").await,
        json!(null),
    );
    let read = world.skill_files(None, None, Request::Read { name: "kit".into() }).await?;
    let Reply::Read { snapshot } = read else { anyhow::bail!("missing skill snapshot") };
    let expected = snapshot.metadata.context("skill metadata")?.version;
    let edited = world
        .skill_files(
            None,
            None,
            Request::Edit {
                name: "kit".into(),
                expected,
                changes: vec![Change::Put { file: File::text("references/note.txt", "ALICE_UPDATED_REFERENCE") }],
            },
        )
        .await?;
    let Reply::Written { metadata } = edited else { anyhow::bail!("skill update conflicted") };
    let updated = original.parent().unwrap().join(format!("kit-{}", metadata.version));
    record(
        "a held skill folder updates from a hub hint without another model read",
        file_is(&updated.join("references/note.txt"), "ALICE_UPDATED_REFERENCE").await,
        json!(null),
    );
    let ready = computer.join("skill-ready");
    let release = computer.join("skill-release");
    let completed = computer.join("skill-completed");
    let command = format!(
        "ALICE_SKILL_LIFETIME [[later: on alice-pc <<'SKILL'\nset -e\ntext=$(fastctx skill kit)\nfolder=$(printf '%s\\n' \"$text\" | sed -n 's/^Skill folder on [^:]*: //p')\nprintf ready > {}\nwhile ! test -f {}; do sleep 0.05; done\ncat \"$folder/references/note.txt\" > {}\nSKILL]]",
        quoted(&ready),
        quoted(&release),
        quoted(&completed)
    );
    world.talk(id.clone(), Some("host".parse()?), command).await?;
    anyhow::ensure!(file_is(&ready, "ready").await, "the held skill job did not start");
    let mut client = fastctx_client::Client::connect(
        &WorldDir::under(computer),
        Peer::Command { resident: Some(Address { machine: machine("host"), resident: id.clone() }), session: None },
    )
    .await?;
    let unauthorized = client
        .call(Call::SkillFiles {
            hub: None,
            account: None,
            request: Request::Delete { name: "kit".into(), expected: metadata.version },
        })
        .await;
    record("a conversation cannot edit or delete the user's skill records", unauthorized.is_err(), json!(null));
    world.skill_files(None, None, Request::Delete { name: "kit".into(), expected: metadata.version }).await?;
    record(
        "deleting a skill preserves the generation used by a running job",
        updated.join("references/note.txt").is_file(),
        json!(null),
    );
    std::fs::write(&release, "")?;
    record(
        "the running script can finish reading its deleted skill",
        file_is(&completed, "ALICE_UPDATED_REFERENCE").await,
        json!(null),
    );
    for _ in 0..240 {
        if !updated.exists() {
            break;
        }
        pause().await;
    }
    record("the deleted generation is removed after its job finishes", !updated.exists(), json!(null));
    Ok(())
}
