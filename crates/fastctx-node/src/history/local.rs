//! Standalone history has the same revision rules without a network account.

use crate::Node;
use fastctx_types::{
    history::{Author, Catalog, Document, HistoryCall, HistoryReply, Scope, stored::Stored},
    ipc::Failure,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) struct Store {
    pub index: fastctx_net::topic_history::Index,
    pub files: Vec<fastctx_net::ledger::File>,
}

pub(crate) fn project(world: &fastctx_types::layout::WorldDir, town: &fastctx_types::TownName) -> io::Result<Store> {
    let mut store = Store { index: Default::default(), files: Vec::new() };
    for (_, entry) in crate::ledger::entries_after(world, town, 0)? {
        store.index.take(&entry).map_err(io::Error::other)?;
        store.files.extend(entry.files.into_iter().filter(|file| file.path.starts_with("history/")));
    }
    Ok(store)
}

pub(crate) fn directory(node: &Node, scope: &Scope) -> PathBuf {
    match scope {
        Scope::Conversation { resident, machine } => node
            .world
            .owner(&node.names(), &fastctx_types::Address { resident: resident.clone(), machine: machine.clone() })
            .path()
            .join("history-records"),
        Scope::Project { .. } => unreachable!("project history is signed"),
    }
}

fn versions(directory: &Path, name: &str) -> Result<Vec<Stored>, String> {
    fastctx_types::history::markdown::check_name(name)?;
    let mut versions = Vec::new();
    match std::fs::read_dir(directory.join(name)) {
        Ok(files) => {
            for file in files {
                let path = file.map_err(|error| error.to_string())?.path();
                if path.extension().is_some_and(|extension| extension == "json") {
                    let stored: Stored =
                        serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
                            .map_err(|error| error.to_string())?;
                    stored.validate()?;
                    versions.push(stored);
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    versions.sort_by_key(|stored| stored.revision.version);
    Ok(versions)
}

pub(crate) async fn call(
    node: &Arc<Node>,
    request: HistoryCall,
    author: Option<Author>,
) -> Result<HistoryReply, Failure> {
    let scope = request.scope().clone();
    let identity = serde_json::to_string(&scope).map_err(failure)?;
    let gate = node.local_history.get_or_insert_with(&identity, || tokio::sync::Mutex::new(()));
    let _local = gate.lock().await;
    let mut documents = Vec::new();
    match &scope {
        Scope::Project { project: town } => {
            let (world, town) = (node.world.clone(), town.clone());
            let store =
                tokio::task::spawn_blocking(move || project(&world, &town)).await.map_err(failure)?.map_err(failure)?;
            for revision in store.index.catalog().documents {
                documents.push(store.index.document(&revision.name, None, &store.files).map_err(failure)?);
            }
        }
        Scope::Conversation { .. } => {
            let directory = directory(node, &scope);
            documents = tokio::task::spawn_blocking(move || {
                let mut documents = Vec::new();
                match std::fs::read_dir(&directory) {
                    Ok(entries) => {
                        for entry in entries {
                            let entry = entry.map_err(|error| error.to_string())?;
                            if !entry.file_type().map_err(|error| error.to_string())?.is_dir() {
                                continue;
                            }
                            let name = entry.file_name().to_string_lossy().to_string();
                            let all = versions(&directory, &name)?;
                            if let Some(stored) = all.last() {
                                documents.push(Document {
                                    revision: stored.revision.clone(),
                                    text: stored.text.clone(),
                                    versions: all.into_iter().map(|stored| stored.revision).collect(),
                                });
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
                Ok::<_, String>(documents)
            })
            .await
            .map_err(failure)?
            .map_err(failure)?;
        }
    }
    let projection = super::local_copy::folder(node, &scope);
    for document in &documents {
        let path = projection.join(format!("{}.md", document.revision.name));
        if document.revision.retired {
            crate::replicas::files::delete(&path).map_err(failure)?;
        } else {
            crate::replicas::files::publish(&path, &document.text, Some(&document.text)).map_err(failure)?;
        }
    }
    let read = |name: &str| {
        documents
            .iter()
            .find(|document| document.revision.name == name)
            .cloned()
            .ok_or_else(|| failure("the history document does not exist"))
    };
    let (name, text, expected, retired) = match request {
        HistoryCall::List { .. } => {
            let mut catalog = Catalog { documents: documents.into_iter().map(|document| document.revision).collect() };
            catalog.sort();
            return Ok(HistoryReply::Catalog { catalog });
        }
        HistoryCall::Read { name, version, .. } => {
            let mut document = read(&name)?;
            if let Some(version) = version {
                let stored = revision(node, &scope, &name, version).await?;
                document.revision = stored.revision;
                document.text = stored.text;
            }
            return Ok(HistoryReply::Document { document: Box::new(document) });
        }
        HistoryCall::Save { name, text, expected, .. } => (name, text, expected, false),
        HistoryCall::Retire { name, expected, .. } => (name.clone(), read(&name)?.text, Some(expected), true),
        HistoryCall::Restore { name, version, .. } => {
            let stored = revision(node, &scope, &name, version).await?;
            let expected = read(&name)?.revision.version;
            (name, stored.text, Some(expected), false)
        }
    };
    let author = author.ok_or_else(|| failure("history writes require attribution"))?;
    let current =
        documents.iter().find(|document| document.revision.name == name).map(|document| document.revision.version);
    if expected != current {
        return Err(failure(fastctx_types::history::conflict(current)));
    }
    let (revision, files) =
        fastctx_net::topic_history::revision_files(&name, &text, expected, retired, author, None).map_err(failure)?;
    match &scope {
        Scope::Project { project } => crate::ledger::write_memory(node, project, files, None).await?,
        Scope::Conversation { .. } => {
            let folder = directory(node, &scope);
            let stored = Stored { revision: revision.clone(), text: text.clone(), submission: None };
            let _gate = node.syncing.lock().await;
            tokio::task::spawn_blocking(move || {
                let current = versions(&folder, &name)?.last().map(|stored| stored.revision.version);
                if current != expected {
                    return Err(fastctx_types::history::conflict(current));
                }
                let path = folder.join(&name).join(format!("{}.json", stored.revision.version));
                fastctx_platform::ensure_private_dir(path.parent().expect("revision parent"))
                    .map_err(|error| error.to_string())?;
                fastctx_platform::publish_new(
                    &path,
                    &serde_json::to_vec(&stored).map_err(|error| error.to_string())?,
                    fastctx_platform::Permissions::Private,
                )
                .map_err(|error| error.to_string())
            })
            .await
            .map_err(failure)?
            .map_err(failure)?;
        }
    }
    let path = projection.join(format!("{}.md", revision.name));
    if retired {
        crate::replicas::files::delete(&path).map_err(failure)?;
    } else {
        crate::replicas::files::publish(
            &path,
            &text,
            documents
                .iter()
                .find(|document| document.revision.name == revision.name)
                .map(|document| document.text.as_str()),
        )
        .map_err(failure)?;
    }
    node.memory_changed();
    Ok(HistoryReply::Saved { revision: Box::new(revision) })
}

async fn revision(node: &Arc<Node>, scope: &Scope, name: &str, version: u64) -> Result<Stored, Failure> {
    match scope {
        Scope::Project { project: town } => {
            let (world, town, name) = (node.world.clone(), town.clone(), name.to_string());
            tokio::task::spawn_blocking(move || {
                let store = project(&world, &town).map_err(|error| error.to_string())?;
                let document = store.index.document(&name, Some(version), &store.files)?;
                Ok::<_, String>(Stored { revision: document.revision, text: document.text, submission: None })
            })
            .await
            .map_err(failure)?
            .map_err(failure)
        }
        Scope::Conversation { .. } => {
            let (folder, name) = (directory(node, scope), name.to_string());
            tokio::task::spawn_blocking(move || {
                versions(&folder, &name)?
                    .into_iter()
                    .find(|stored| stored.revision.version == version)
                    .ok_or_else(|| "the history revision does not exist".to_string())
            })
            .await
            .map_err(failure)?
            .map_err(failure)
        }
    }
}
