//! Causal receipt collections grow by one indexed member, never by rewriting
//! all previous children. The partial-exit snapshot is immutable after sealing.
use super::*;
use crate::{
    job_metadata::Job,
    records::{self, Source},
};

fn child_key(child: &Child) -> String {
    crate::speech::fixed_id(
        "child",
        &[&serde_json::to_string(&(&child.completed, &child.origin, child.through_exit))
            .expect("serializable receipt")],
    )
    .to_string()
}
pub(crate) fn prepare(path: &Path) -> io::Result<Job> {
    let job = Job::open(path)?;
    for (kind, field, extension) in [("waited", None, "waited"), ("exit-children", Some("children"), "exit-receipt")] {
        job.prepare_file(kind, path.with_extension(extension), move |source, push| {
            records::legacy::raw_array::<Child>(source, field, &mut |child, body| {
                push(Source::new(child_key(&child), source.to_path_buf(), body)?)
            })
        })?;
    }
    for (kind, field, extension) in
        [("consumed", None, "consumed"), ("exit-consumed", Some("consumed"), "exit-receipt")]
    {
        job.prepare_file(kind, path.with_extension(extension), move |source, push| {
            records::legacy::array::<NoticeId>(source, field, &mut |id| {
                push(Source::new(id.to_string(), source.to_path_buf(), serde_json::to_vec(&id)?)?)
            })
        })?;
    }
    job.prepare_file("exit-frozen", path.with_extension("exit-receipt"), |source, push| {
        for field in ["children", "consumed"] {
            if records::legacy::field::<serde::de::IgnoredAny>(source, field)?.is_none() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "partial exit receipt has a missing field"));
            }
        }
        push(Source::new("value".into(), source.to_path_buf(), b"true".to_vec())?)
    })?;
    Ok(job)
}
pub(super) fn children(path: &Path) -> io::Result<Vec<Child>> {
    prepare(path)?.all("waited")
}
pub(super) fn consumed(path: &Path) -> io::Result<Vec<NoticeId>> {
    prepare(path)?.all("consumed")
}
pub(super) fn bind(path: &Path, child: Child) -> io::Result<()> {
    let job = prepare(path)?;
    job.change("waited", serde_json::to_vec(&child)?.len(), move |rows, family| {
        let key = child_key(&child);
        if rows.get_raw(family, &key)?.is_none() {
            rows.put(family, &key, &child)?;
        }
        Ok(())
    })
}
pub(super) fn inherit(path: &Path, receipts: &[NoticeId]) -> io::Result<()> {
    let job = prepare(path)?;
    for chunk in receipts.chunks(16) {
        let chunk = chunk.to_vec();
        job.change("consumed", chunk.len() * 128, move |rows, family| {
            for id in chunk {
                rows.put(family, id.as_str(), &id)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}
pub(super) fn frozen(path: &Path) -> io::Result<Option<ExitReceipt>> {
    let job = prepare(path)?;
    if !job.get::<bool>("exit-frozen")?.unwrap_or(false) {
        return Ok(None);
    }
    Ok(Some(ExitReceipt { children: job.all("exit-children")?, consumed: job.all("exit-consumed")? }))
}
pub(super) fn freeze(path: &Path, receipt: ExitReceipt) -> io::Result<()> {
    let job = prepare(path)?;
    if job.get::<bool>("exit-frozen")?.unwrap_or(false) {
        return Ok(());
    }
    for chunk in receipt.children.chunks(16) {
        let chunk = chunk.to_vec();
        job.change("exit-children", serde_json::to_vec(&chunk)?.len(), move |rows, family| {
            for child in chunk {
                rows.put(family, &child_key(&child), &child)?;
            }
            Ok(())
        })?;
    }
    for chunk in receipt.consumed.chunks(16) {
        let chunk = chunk.to_vec();
        job.change("exit-consumed", chunk.len() * 128, move |rows, family| {
            for id in chunk {
                rows.put(family, id.as_str(), &id)?;
            }
            Ok(())
        })?;
    }
    job.put("exit-frozen", &true)
}
