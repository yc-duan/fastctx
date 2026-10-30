//! Folder edits are atomic across the skill, a rename and the index.
use crate::{report::Report, world::World};
use fastctx_types::{
    account_records::{Request as Records, Response},
    skill_files::{Change, File, Reply, Request},
    skills::{Index, Skill},
};
use std::time::Duration;
fn main_file(name: &str, description: &str, body: &str) -> File {
    File::text("SKILL.md", &format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"))
}
fn written(reply: Reply) -> anyhow::Result<u64> {
    match reply {
        Reply::Written { metadata } => Ok(metadata.version),
        _ => anyhow::bail!("expected a successful skill write"),
    }
}

pub async fn exercise(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let a = world.machine("alpha").link();
    let b = world.machine("beta").link();
    let account = &world.account;
    let limits = a.skill_files(account, Request::Limits).await.map_err(anyhow::Error::msg)?;
    report.check(
        matches!(limits,Reply::Limits{limits}if limits.account_bytes>0&&limits.files>0),
        "skill limits derive from the hub's available resources",
        "",
    );
    let legacy = Skill {
        name: "legacy".into(),
        description: "legacy only".into(),
        instructions: "old body".into(),
        files: None,
    };
    let legacy = a
        .account_records(
            account,
            Records::Write {
                kind: "skill".into(),
                key: "legacy".into(),
                expected: None,
                body: serde_json::to_value(legacy)?,
            },
        )
        .await
        .map_err(anyhow::Error::msg)?;
    report.check(matches!(legacy, Response::Written { .. }), "an older single-body writer still works", "");
    let legacy = a.skill_files(account, Request::Read { name: "legacy".into() }).await.map_err(anyhow::Error::msg)?;
    report.check(matches!(legacy,Reply::Read{snapshot}if snapshot.skill.as_ref().is_some_and(|skill|skill.files.as_ref().is_some_and(|files|files.len()==1&&files[0].path=="SKILL.md"))),"legacy skills are read as a folder with SKILL.md","");
    let mut script = File::text("scripts/check.sh", "#!/bin/sh\nprintf '%s\\n' hello\n");
    script.executable = true;
    let files = vec![
        main_file("kit", "initial description", "initial body"),
        script.clone(),
        File { path: "empty".into(), data: None, executable: false },
    ];
    let v1 = written(
        a.skill_files(account, Request::Write { name: "kit".into(), expected: None, files })
            .await
            .map_err(anyhow::Error::msg)?,
    )?;
    let got = b
        .skill_files(account, Request::ReadFile { name: "kit".into(), path: "scripts/check.sh".into() })
        .await
        .map_err(anyhow::Error::msg)?;
    report.check(
        matches!(got,Reply::File{version,file:Some(file)}if version==v1&&file==script),
        "auxiliary bytes and executable metadata round-trip exactly",
        "",
    );
    let edit = Request::Edit {
        name: "kit".into(),
        expected: v1,
        changes: vec![Change::Put { file: main_file("kit", "updated description", "updated body") }],
    };
    let (left, right) = tokio::join!(a.skill_files(account, edit.clone()), b.skill_files(account, edit));
    let results = [left.map_err(anyhow::Error::msg)?, right.map_err(anyhow::Error::msg)?];
    report.check(
        results.iter().filter(|r| matches!(r, Reply::Written { .. })).count() == 1
            && results.iter().filter(|r| matches!(r, Reply::Conflict { .. })).count() == 1,
        "two editors using one read revision cannot silently overwrite each other",
        "",
    );
    let v2 = results
        .into_iter()
        .find_map(|r| match r {
            Reply::Written { metadata } => Some(metadata.version),
            _ => None,
        })
        .expect("one accepted");
    let current = b.skill_files(account, Request::Read { name: "kit".into() }).await.map_err(anyhow::Error::msg)?;
    report.check(matches!(current,Reply::Read{snapshot}if snapshot.skill.as_ref().is_some_and(|skill|skill.description=="updated description"&&skill.files.as_ref().is_some_and(|files|files.contains(&script)))),"a single-file edit preserves auxiliary files","");
    let index = b
        .account_records(account, Records::Read { kind: "skill-index".into(), key: "index".into() })
        .await
        .map_err(anyhow::Error::msg)?;
    let Response::Read { record: Some(index) } = index else { anyhow::bail!("missing index") };
    let mut catalog: Index = serde_json::from_value(index.body)?;
    report.check(
        catalog.skills.iter().any(|item| item.name == "kit" && item.description == "updated description"),
        "the index is committed with the folder",
        "",
    );
    catalog.skills.iter_mut().find(|item| item.name == "kit").unwrap().description = "stale description".into();
    let stale = b
        .account_records(
            account,
            Records::Write {
                kind: "skill-index".into(),
                key: "index".into(),
                expected: Some(index.metadata.version),
                body: serde_json::to_value(catalog)?,
            },
        )
        .await;
    report.check(stale.is_err(), "an old client cannot put a stale description back into the latest index", "");
    let orphan=b.account_records(account,Records::Write{kind:"skill".into(),key:"renamed-by-old-client".into(),expected:None,body:serde_json::json!({"name":"renamed-by-old-client","description":"old editor","instructions":"partial rename"})}).await;
    report.check(orphan.is_err(), "a legacy rename is refused before it can create a partial folder", "");
    for path in ["../escape", "/absolute", "CON.txt", "A/../x", "COM¹.txt", "Ａ/．．/x"] {
        let invalid = a
            .skill_files(
                account,
                Request::Edit {
                    name: "kit".into(),
                    expected: v2,
                    changes: vec![Change::Put { file: File::text(path, "bad") }],
                },
            )
            .await;
        report.check(invalid.is_err(), "nonportable or escaping skill paths are refused", path);
    }
    let collision = a
        .skill_files(
            account,
            Request::Edit {
                name: "kit".into(),
                expected: v2,
                changes: vec![Change::Put { file: File::text("SCRIPTS/check.sh", "collision") }],
            },
        )
        .await;
    report.check(collision.is_err(), "case-insensitive path collisions are refused on the hub", "");
    let directory_collision = a
        .skill_files(
            account,
            Request::Edit {
                name: "kit".into(),
                expected: v2,
                changes: vec![Change::Put { file: File::text("SCRIPTS/different.sh", "collision") }],
            },
        )
        .await;
    report.check(directory_collision.is_err(), "implicit directory case collisions are refused", "");
    let renamed = written(
        a.skill_files(
            account,
            Request::Edit {
                name: "kit".into(),
                expected: v2,
                changes: vec![Change::Put { file: main_file("renamed", "updated description", "updated body") }],
            },
        )
        .await
        .map_err(anyhow::Error::msg)?,
    )?;
    let old = b.skill_files(account, Request::Read { name: "kit".into() }).await.map_err(anyhow::Error::msg)?;
    report.check(
        matches!(old,Reply::Read{snapshot}if snapshot.skill.is_none()),
        "rename retires the old skill in the same transaction",
        "",
    );
    world.hub.restart(Duration::from_millis(100)).await?;
    anyhow::ensure!(world.all_online(Duration::from_secs(60)).await, "members did not reconnect");
    let saved = b
        .skill_files(account, Request::ReadFile { name: "renamed".into(), path: "scripts/check.sh".into() })
        .await
        .map_err(anyhow::Error::msg)?;
    report.check(
        matches!(saved,Reply::File{file:Some(file),..}if file==script),
        "the renamed folder survives a real hub restart",
        "",
    );
    written(
        a.skill_files(account, Request::Delete { name: "renamed".into(), expected: renamed })
            .await
            .map_err(anyhow::Error::msg)?,
    )?;
    let listed = b.skill_files(account, Request::List).await.map_err(anyhow::Error::msg)?;
    report.check(
        matches!(listed,Reply::Listed{skills}if !skills.iter().any(|skill|skill.name=="renamed")),
        "deletion removes the folder from the index",
        "",
    );
    Ok(())
}
