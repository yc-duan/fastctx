//! Phone uploads and command copies follow the actual working computer.
use super::*;
use fastctx_types::{
    ResidentId,
    attachments::{UploadFile, UploadTarget},
    file_read::{FileRange, ReadFileRequest},
};
use sha2::{Digest, Sha256};
async fn bytes(mut reading: fastctx_client::file_read::Reading) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(part) = reading.next().await? {
        match part {
            fastctx_client::file_read::Read::Opened(_) => bytes.clear(),
            fastctx_client::file_read::Read::Bytes(part) => bytes.extend(part),
        }
    }
    anyhow::ensure!(reading.complete().is_some(), "file stream ended without verified completion");
    Ok(bytes)
}
pub(super) async fn exercise(world: &World, host: &Path, plan: &Path, id: &ResidentId) -> Result<()> {
    let data = b"ASSISTED_ATTACHMENT_CANARY\n";
    let receipt = world
        .upload(
            UploadTarget::Resident { resident: id.clone() },
            Some("host".parse()?),
            UploadFile {
                name: "sample.txt".into(),
                media_type: "text/plain".into(),
                size: data.len() as u64,
                sha256: format!("{:x}", Sha256::digest(data)),
            },
            data.as_slice(),
        )
        .await?;
    record(
        "a phone's non-image attachment is stored on its selected working computer",
        Path::new(&receipt.path).starts_with(plan.parent().unwrap()) && std::fs::read(&receipt.path)? == data,
        json!(receipt.path),
    );
    record(
        "the phone can read the verified original",
        bytes(world.read_attachment(receipt.clone())).await? == data,
        json!(null),
    );
    let original_plan = std::fs::read_to_string(plan)?;
    let changed_plan = format!("{original_plan}ATTACH_CONTEXT\n");
    std::fs::write(plan, &changed_plan)?;
    anyhow::ensure!(plan_is(world, id, &changed_plan).await, "attachment context did not sync");
    let relative = fastctx_types::session::attachments::place(&receipt);
    let command =
        format!("ALICE_ATTACHMENT [[run: on alice-two <<'ATTACH'\ncat \"$FASTCTX_SCRATCH/{relative}\"\nATTACH]]");
    world
        .talk_with_attachments(
            id.clone(),
            Some("host".parse()?),
            command,
            vec![receipt.clone()],
            Some("alice-two".parse()?),
            Some("placement-attachment".into()),
        )
        .await?;
    anyhow::ensure!(idle(host, id.as_str()).await, "attachment turn did not finish");
    let shown = super::controls::shown(world, id).await?;
    record(
        "a changed context block does not hide or corrupt attachment history",
        shown.iter().any(|event| {
            matches!(&event.happened,
            fastctx_types::session::Happened::Heard { text, message: Some(message), .. }
                if text.starts_with("ALICE_ATTACHMENT") && message.attachments.len() == 1)
        }),
        json!(null),
    );
    std::fs::write(plan, &original_plan)?;
    anyhow::ensure!(plan_is(world, id, &original_plan).await, "the attachment fixture did not restore its plan");
    let copy = plan.parent().unwrap().join("scratch").join(relative);
    let read = world.read_file(
        Some("alice-two".parse()?),
        ReadFileRequest { path: copy.to_string_lossy().into_owned(), range: FileRange::All },
    );
    record("the command received its attachment in the working scratch", bytes(read).await? == data, json!(copy));
    record(
        "the assisting host has no attachment original or scratch copy",
        !host.join("world/conversations").join(id.as_str()).join("scratch/attachments").exists(),
        json!(null),
    );
    Ok(())
}
