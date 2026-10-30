//! A page's single line on the map and in the page list.

pub fn line(page: &fastctx_types::pages::PageSummary) -> String {
    let current = &page.current;
    let by = crate::town::writer(&current.by, current.on.as_ref());
    format!(
        "{}: {} (version {}{}; {}, {})",
        page.page,
        current.description,
        current.version,
        if current.withdrawn { "; withdrawn" } else { "" },
        match &current.on {
            Some(on) => format!("{by} on {on}"),
            None => by,
        },
        current.at
    )
}
