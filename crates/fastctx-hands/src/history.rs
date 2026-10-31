//! A bounded topic catalog, followed by the shared writing contract.

use fastctx_types::history::{CATALOG_CONTEXT_BYTES, Catalog, words};

pub fn catalog(catalog: &Catalog, path: &str) -> String {
    let active: Vec<_> = catalog.documents.iter().filter(|revision| !revision.retired).collect();
    let mut lines: Vec<String> =
        active.iter().map(|revision| format!("{}: {}", revision.name, revision.description)).collect();
    let mut bytes = lines.iter().map(|line| line.len() + 1).sum::<usize>();
    for (line, revision) in lines.iter_mut().zip(&active).rev() {
        if bytes <= CATALOG_CONTEXT_BYTES {
            break;
        }
        bytes -= line.len() - revision.name.len();
        *line = revision.name.clone();
    }
    if bytes <= CATALOG_CONTEXT_BYTES {
        return lines.join("\n");
    }
    let quoted = format!("'{}'", path.replace('\'', "'\\''"));
    let mut kept = Vec::new();
    let mut used = 0;
    for line in &lines {
        let omitted = lines.len() - kept.len();
        let tail = format!("{omitted} more topics. List all descriptions with rg -H '^description:' {quoted}");
        if used + line.len() + 1 + tail.len() > CATALOG_CONTEXT_BYTES {
            break;
        }
        kept.push(line.as_str());
        used += line.len() + 1;
    }
    let omitted = lines.len() - kept.len();
    let tail = format!("{omitted} more topics. List all descriptions with rg -H '^description:' {quoted}");
    kept.push(&tail);
    kept.join("\n")
}

pub fn context(catalogue: &Catalog, path: &str, machine: &str, project: bool, read_only: bool) -> String {
    let mut template = if read_only {
        let intro =
            words::TEMPLATE.split("\nThe history is how this work carries on").next().expect("history introduction");
        format!("{intro}\n{}", words::READ_ONLY)
    } else {
        words::TEMPLATE.to_string()
    };
    if !project {
        template = template.replace("This project's history", "This conversation's history");
    }
    template
        .replace("<path>", path)
        .replace("<machine>", machine)
        .replace("{{ name: description, one a line }}", &catalog(catalogue, path))
}

pub fn changed(revision: &fastctx_types::history::HistoryRevision) -> String {
    let by = crate::town::writer(&revision.author.stamp.by, revision.author.stamp.on.as_ref());
    if revision.retired {
        format!("{by} retired the history topic {}.", revision.name)
    } else {
        format!("{by} updated the history topic {}: {}", revision.name, revision.description)
    }
}
