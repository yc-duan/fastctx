//! A page's single line on the map and in the page list.

pub fn line(page: &fastctx_types::pages::PageSummary) -> String {
    let current = &page.current;
    format!(
        "{}: {} (version {}{}; {} on {}, {})",
        page.page,
        current.description,
        current.version,
        if current.withdrawn { "; withdrawn" } else { "" },
        crate::town::writer(&current.by, &current.on),
        current.on,
        current.at
    )
}
