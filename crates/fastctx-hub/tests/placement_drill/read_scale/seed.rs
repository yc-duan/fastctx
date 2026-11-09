//! Synthetic resident registration in the same on-disk formats the node opens.
use super::*;

pub(super) fn prepare(base: &Path, fixture: &Path, manifest: &Value) -> Result<()> {
    let root = base.join("host");
    let world = root.join("world");
    let home = base.join("claude");
    std::fs::create_dir_all(&home)?;
    let now = jiff::Timestamp::now();
    for conversation in manifest["conversations"].as_array().context("conversations")? {
        let resident = conversation["resident"].as_str().context("resident")?;
        let session = conversation["session"].as_str().context("session")?;
        let directory = world.join("conversations").join(resident);
        let origin = match conversation["colleague_of"].as_str() {
            Some(parent) => json!({"kind":"colleague","asked_by":{"machine":"host","resident":parent},"label":session}),
            None => json!({"kind":"user"}),
        };
        let about: fastctx_types::residents::About = serde_json::from_value(json!({
            "id":resident,"host":{"agent":"claude","client":"scale-drill","version":"0"},
            "machine":"host","born_at":now,"origin":origin,"session":session,"world_started":true,"home":home,
            "acting":{"kind":"here"}
        }))?;
        save(&directory.join("about"), &serde_json::to_value(about)?)?;
        let state: fastctx_types::residents::Now = serde_json::from_value(json!({
            "activity":"idle","since":now,"turn":null,"answering":[],"trouble":null,"title":session
        }))?;
        save(&directory.join("now"), &serde_json::to_value(state)?)?;
        let exchange = world.join("claude").join(session);
        save(&exchange.join("resident.json"), &json!(resident))?;
        save(&exchange.join("control/checkpoints.json"), &json!({"full":false,"turns":{}}))?;
        save(
            &exchange.join("session.json"),
            &json!({"session":session,"at":now,"process":null,
            "inbox_address":null,"entrypoint":"scale-drill","model":null,"version":"fixture"}),
        )?;
        save(
            &exchange.join("transcript.json"),
            &json!({"path":fixture.join(conversation["path"].as_str().context("source path")?)}),
        )?;
    }
    let conversations = manifest["conversations"].as_array().context("conversations")?;
    for index in 0..manifest["jobs"].as_u64().context("jobs")? {
        let resident = conversations[index as usize % conversations.len()]["resident"].as_str().context("resident")?;
        let directory = world.join("conversations").join(resident);
        let id = format!("j-{index:06x}");
        let output = directory.join("out").join(format!("{id}.log"));
        std::fs::create_dir_all(output.parent().context("output parent")?)?;
        let length = if index == 0 {
            std::fs::hard_link(fixture.join("jobs/output.log"), &output)?;
            manifest["settings"]["output_bytes"].as_u64().context("output size")?
        } else {
            std::fs::write(&output, b"synthetic job output\n")?;
            21
        };
        let record: fastctx_types::run::JobRecord = serde_json::from_value(json!({
            "id":id,"machine":"host","owner":{"machine":"host","resident":resident},"town":null,
            "script":"printf synthetic","cwd":base.join("user"),"output":output,"started_at":now,
            "supervisor":null,"pid":null,"exit":{"status":{"code":0},"at":now},
            "output_ended_at":now,"output_drained":{"stdout":length,"stderr":0},"remaining":0,"output_lost":null,
            "end":{"at":now,"how":{"kind":"finished"}},"ends_with_node":false
        }))?;
        save(&directory.join("jobs").join(format!("{id}.json")), &serde_json::to_value(record)?)?;
    }
    let places = fastctx_install::apply::Places {
        root: root.clone(),
        world: world.clone(),
        codex_home: base.join("codex"),
        claude_home: home.clone(),
        claude_config_dir: Some(home.clone()),
        claude: None,
        fastctx: KERNEL.get().expect("kernel").clone(),
        workbench: Some(false),
    };
    fastctx_install::apply::apply(
        &places,
        fastctx_install::apply::Host::Claude.only(),
        fastctx_install::codex::Approval::Require,
    )?;
    save(&world.join("agent-scan.json"), &json!({"version":1,"workbench_seeded":true,"claude":{"workbench":[home]}}))?;
    Ok(())
}

fn save(path: &Path, value: &Value) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("parent")?)?;
    std::fs::write(path, serde_json::to_vec(value)?)?;
    Ok(())
}
