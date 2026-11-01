//! A frozen request and its observation root are one immutable publication.
use super::{super::reviewer::Materials, failure};
use crate::{
    records::{self, maps},
    turn_evidence::review::Seen,
};
use fastctx_storage::{Error, ErrorKind, Priority};
use fastctx_types::{ipc::Failure, layout::ConversationDir};
use std::io;

const FAMILY: &str = "goal-review-materials";
const MAP: &str = "goal-review-observations";

fn decode(values: maps::Values) -> io::Result<Seen> {
    values
        .into_iter()
        .map(|(key, value)| {
            String::from_utf8(value)
                .map(|value| (key, value))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        })
        .collect::<io::Result<_>>()
        .map(Seen)
}

pub(super) async fn stage(directory: &ConversationDir, seen: Seen) -> Result<maps::Staged, Failure> {
    let (database, scope) = records::locate(directory).map_err(failure)?;
    let values =
        tokio::task::spawn_blocking(move || seen.0.into_iter().map(|(key, value)| (key, value.into_bytes())).collect())
            .await
            .map_err(failure)?;
    maps::stage(database, scope, MAP.into(), values, Priority::Background, |_| Ok(()), |_, staged| Ok(staged.clone()))
        .await
        .map(|(staged, ())| staged)
        .map_err(failure)
}

pub(super) async fn observations(directory: ConversationDir, owner: String) -> Result<Seen, Failure> {
    let reading = directory.clone();
    let key = owner.clone();
    let (seen, imported) = tokio::task::spawn_blocking(move || -> io::Result<_> {
        if let Some(values) = maps::load(&reading, MAP, &key)? {
            return Ok((decode(values)?, true));
        }
        let bytes = std::fs::read(reading.books().join("goal-reviews").join(&key).join("observations.json"))?;
        Ok((serde_json::from_slice(&bytes)?, false))
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    if imported {
        return Ok(seen);
    }
    let staged = stage(&directory, seen).await?;
    let reading = directory.clone();
    let key = owner.clone();
    tokio::task::spawn_blocking(move || {
        let pin = staged.clone();
        records::write_then_at(
            &reading,
            Priority::Background,
            512,
            move |rows| {
                if !maps::exists(rows, MAP, &key)? {
                    maps::link(rows, &key, &staged)?;
                }
                maps::finish(rows, &staged)
            },
            move |_| drop(pin),
        )
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    tokio::task::spawn_blocking(move || {
        maps::load(&directory, MAP, &owner)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the imported observation root is missing"))
            .and_then(decode)
    })
    .await
    .map_err(failure)?
    .map_err(failure)
}

pub(super) async fn cached(directory: ConversationDir, work: String) -> Result<Option<Materials>, Failure> {
    let reading = directory.clone();
    let key = work.clone();
    let (cached, legacy) = tokio::task::spawn_blocking(move || -> Result<_, Failure> {
        if let Some(materials) = records::read(&reading, |rows| {
            let materials: Option<Materials> = rows.get(FAMILY, &key)?;
            if materials.as_ref().is_some_and(needs_observations) && !maps::exists(rows, MAP, &key)? {
                return Err(Error::with_kind(ErrorKind::Corrupt, "a frozen review has lost its observation root"));
            }
            Ok(materials)
        })
        .map_err(failure)?
        {
            return Ok((Some(materials), false));
        }
        match std::fs::read(reading.books().join("goal-reviews").join(&key).join("materials.json")) {
            Ok(bytes) => {
                Ok((Some(super::material_size::cached(serde_json::from_slice(&bytes).map_err(failure)?)?), true))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((None, false)),
            Err(error) => Err(failure(error)),
        }
    })
    .await
    .map_err(failure)??;
    let Some(materials) = cached else {
        return Ok(None);
    };
    if !legacy {
        return Ok(Some(materials));
    }
    let staged = if needs_observations(&materials) {
        let reading = directory.clone();
        let key = work.clone();
        let seen = tokio::task::spawn_blocking(move || -> io::Result<Seen> {
            Ok(serde_json::from_slice(&std::fs::read(
                reading.books().join("goal-reviews").join(key).join("observations.json"),
            )?)?)
        })
        .await
        .map_err(failure)?
        .map_err(failure)?;
        Some(stage(&directory, seen).await?)
    } else {
        None
    };
    install(directory, work, materials, staged).await.map(Some)
}

fn needs_observations(materials: &Materials) -> bool {
    materials.world_actions.as_ref().is_some_and(|evidence| evidence.snapshot.is_some())
}

pub(super) async fn install(
    directory: ConversationDir,
    work: String,
    materials: Materials,
    staged: Option<maps::Staged>,
) -> Result<Materials, Failure> {
    tokio::task::spawn_blocking(move || {
        let bytes = records::encode(&materials).map_err(io::Error::other)?;
        if bytes.len() > super::material_size::MAX_BYTES {
            return Err(io::Error::other("a frozen review exceeds its projection budget"));
        }
        let pin = staged.clone();
        records::write_then_at(
            &directory,
            Priority::Interactive,
            bytes.len() + 512,
            move |rows| {
                let actual = match rows.get::<Materials>(FAMILY, &work)? {
                    Some(previous) => {
                        if previous.account != materials.account
                            || previous.conversation != materials.conversation
                            || previous.goal_id != materials.goal_id
                        {
                            return Err(Error::with_kind(ErrorKind::Invalid, "a review work ID changed its owner"));
                        }
                        previous
                    }
                    None => {
                        if needs_observations(&materials) {
                            maps::link(
                                rows,
                                &work,
                                staged.as_ref().ok_or_else(|| {
                                    Error::with_kind(
                                        ErrorKind::Invalid,
                                        "a frozen review needs its observation snapshot",
                                    )
                                })?,
                            )?;
                        } else {
                            maps::delete_root(rows, MAP, &work)?;
                        }
                        rows.put_raw(FAMILY, &work, &bytes)?;
                        materials
                    }
                };
                if let Some(staged) = staged {
                    maps::finish(rows, &staged)?;
                }
                Ok(actual)
            },
            move |_| drop(pin),
        )
    })
    .await
    .map_err(failure)?
    .map_err(failure)
}
