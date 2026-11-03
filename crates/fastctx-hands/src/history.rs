//! The topic catalog, every topic's name and description, followed by the
//! shared writing contract.

use fastctx_types::history::{Catalog, words};

pub fn catalog(catalog: &Catalog) -> String {
    catalog
        .documents
        .iter()
        .filter(|revision| !revision.retired)
        .map(|revision| format!("{}: {}", revision.name, revision.description))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn context(catalogue: &Catalog, path: &str, machine: &str, project: bool) -> String {
    let mut template = words::TEMPLATE.to_string();
    if !project {
        template = template.replace("This project's history", "This conversation's history");
    }
    template
        .replace("<path>", path)
        .replace("<machine>", machine)
        .replace("{{ name: description, one a line }}", &catalog(catalogue))
}

pub fn changed(revision: &fastctx_types::history::HistoryRevision) -> String {
    let by = crate::town::writer(&revision.author.stamp.by, revision.author.stamp.on.as_ref());
    if revision.retired {
        format!("{by} retired the history topic {}.", revision.name)
    } else {
        format!("{by} updated the history topic {}: {}", revision.name, revision.description)
    }
}
