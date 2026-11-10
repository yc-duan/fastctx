//! Product deletion crosses the node, named hub question, job and record store.

mod records;

use super::*;
use fastctx_types::{
    ResidentId,
    account_records::Record,
    history::{HistoryCall, Scope},
};

const RAN: &str = "job.ran.ask/delete-conversation-records";

async fn ran_after(base: &Path, after: jiff::Timestamp, minimum: u64, name: &str) -> Result<u64> {
    for _ in 0..120 {
        let health = std::fs::read(base.join("hub/health.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if let Some(health) = health
            && health["at"].as_str().and_then(|at| at.parse::<jiff::Timestamp>().ok()).is_some_and(|at| at > after)
        {
            let count = health["jobs"]["timings"]
                .as_array()
                .context("hub job timings")?
                .iter()
                .find(|timing| timing["name"] == name)
                .and_then(|timing| timing["count"].as_u64())
                .unwrap_or(0);
            if count >= minimum {
                return Ok(count);
            }
        }
        pause().await;
    }
    anyhow::bail!("the hub did not publish fresh job timings")
}

fn documents(base: &Path, resident: &ResidentId) -> Result<Vec<(PathBuf, Record)>> {
    let mut found = Vec::new();
    for account in std::fs::read_dir(base.join("hub/account-records"))? {
        for incarnation in std::fs::read_dir(account?.path())? {
            for file in std::fs::read_dir(incarnation?.path())? {
                let path = file?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let Ok(record) = serde_json::from_slice::<Record>(&std::fs::read(&path)?) else { continue };
                let metadata = &record.metadata;
                if (metadata.kind == fastctx_types::plan::KIND && metadata.key == resident.as_str())
                    || (metadata.kind == fastctx_types::history::RECORD_KIND
                        && metadata.key.strip_prefix(resident.as_str()).is_some_and(|suffix| suffix.starts_with('/')))
                {
                    found.push((path, record));
                }
            }
        }
    }
    Ok(found)
}

pub(super) async fn exercise(base: &Path, world: &World, host: &Path, home: &Path) -> Result<()> {
    one(base, world, host, home, false).await?;
    #[cfg(feature = "submission-drill")]
    one(base, world, host, home, true).await?;
    records::exercise(base, world).await?;
    Ok(())
}

async fn one(base: &Path, world: &World, host: &Path, home: &Path, slow: bool) -> Result<()> {
    let tag = if slow { "slow" } else { "normal" };
    let opened = world
        .open_with(
            "codex".parse()?,
            None,
            Some("host".parse()?),
            format!("HUB_DELETE_{tag}"),
            Default::default(),
            fastctx_types::agents::Choices { home: Some(home.to_path_buf()), ..Default::default() },
        )
        .await?;
    let id = opened.to.resident;
    anyhow::ensure!(idle_after(host, id.as_str(), None).await, "the {tag} deletion fixture did not settle");
    let scope = Scope::Conversation { resident: id.clone(), machine: machine("host") };
    let note = "---\nname: submission-delete\ndescription: Hub deletion fixture\n---\nDELETE_HISTORY\n";
    world
        .history(HistoryCall::Save {
            scope: scope.clone(),
            name: "submission-delete".into(),
            text: note.into(),
            expected: None,
        })
        .await?;
    world
        .history(HistoryCall::Save {
            scope: scope.clone(),
            name: "submission-delete".into(),
            text: note.replace("DELETE_HISTORY", "DELETE_HISTORY_V2"),
            expected: Some(1),
        })
        .await?;
    for text in ["DELETE_PLAN", "DELETE_PLAN_V2"] {
        let plan = world.plan_read(None, None, id.clone(), Some(machine("host"))).await?;
        let written =
            world.plan_write(None, None, id.clone(), plan.version(), text.into(), Some(machine("host"))).await?;
        let fastctx_types::plan::Result::Page { page } = written else {
            anyhow::bail!("the {tag} plan fixture conflicted");
        };
        anyhow::ensure!(
            page.body.is_some_and(|body| body.markdown == text),
            "the {tag} plan fixture did not commit its text"
        );
    }
    let before_documents = documents(base, &id)?;
    anyhow::ensure!(
        before_documents.len() == 2
            && before_documents
                .iter()
                .all(|(path, record)| !record.metadata.deleted && path.with_extension("versions").is_dir()),
        "both live hub documents must have old versions before deletion"
    );
    let before = ran_after(base, jiff::Timestamp::now(), 0, RAN).await?;
    let delay = base.join("hub/drill-slow-record-deletion");
    if slow {
        std::fs::write(&delay, id.as_str())?;
    }
    let began = std::time::Instant::now();
    let answer = world.forget_conversation(id.clone(), "host".parse()?).await?;
    let directory = host.join("world/conversations").join(id.as_str());
    for _ in 0..600 {
        if !directory.exists() {
            break;
        }
        pause().await;
    }
    let finished = jiff::Timestamp::now();
    let after = ran_after(base, finished, before + 1, RAN).await?;
    let records = documents(base, &id)?;
    let gone = records.len() == 2
        && records.iter().all(|(path, record)| {
            record.metadata.deleted && record.body.is_null() && !path.with_extension("versions").exists()
        });
    let detail = json!({"resident":id,"ran_before":before,"ran_after":after,"node_directory_gone":!directory.exists(),"hub_documents_erased":gone,"answer":answer});
    record(
        &format!("{tag} product deletion runs one hub job, erases plan/history versions and finishes on the node"),
        after == before + 1 && gone && !directory.exists(),
        detail,
    );
    if slow {
        std::fs::remove_file(delay)?;
        let log = std::fs::read_to_string(base.join("hub/hub.log"))?;
        let lines: Vec<_> = log
            .lines()
            .filter(|line| line.contains("submission-drill answered") && line.contains(&format!("resident={id} ")))
            .collect();
        let names: Vec<_> = lines
            .iter()
            .filter_map(|line| line.split(" name=").nth(1).and_then(|part| part.split(' ').next()))
            .collect();
        let accepted = lines.iter().any(|line| line.contains(" accepted=true "));
        let done = lines.last().is_some_and(|line| line.ends_with("done=true"));
        let same = names.len() >= 2 && names[0] != "unnamed" && names.iter().all(|name| *name == names[0]);
        record(
            "slow hub deletion answers accepted, is asked again under the same name and returns its result once",
            accepted && done && same && after == before + 1 && began.elapsed() >= Duration::from_secs(45),
            json!({"questions":lines,"same_name":same,"elapsed_ms":began.elapsed().as_millis(),"ran_delta":after-before}),
        );
    }
    Ok(())
}
