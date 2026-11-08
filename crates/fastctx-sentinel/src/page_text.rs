//! A web page as text a model reads: what its scripts, styles and markup
//! hold left out, its common entities decoded, a line for each block and
//! its runs of blank space closed up. What is not HTML is read as it is.

/// Elements whose content is never text to read.
const UNREAD: &[&str] = &["script", "style", "noscript", "svg", "template", "head"];
/// Elements that stand on lines of their own.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "tr",
    "td",
    "th",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "section",
    "article",
    "header",
    "footer",
    "table",
    "ul",
    "ol",
    "pre",
    "blockquote",
    "hr",
    "dt",
    "dd",
    "title",
    "main",
    "nav",
    "aside",
    "form",
];

/// `body` as text, as its content type and its start say it is.
pub(crate) fn of(content_type: Option<&str>, body: &str) -> String {
    let start = body.trim_start().get(..15).unwrap_or_default().to_ascii_lowercase();
    let html = content_type.is_some_and(|kind| kind.contains("html"))
        || start.starts_with("<!doctype html")
        || start.starts_with("<html");
    if html { text(body) } else { body.to_string() }
}

/// The text of an HTML document.
fn text(html: &str) -> String {
    let mut out = String::new();
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(open) = html[at..].find('<').map(|found| at + found) {
        out.push_str(&decoded(&html[at..open]));
        if lower[open..].starts_with("<!--") {
            at = lower[open..].find("-->").map_or(html.len(), |end| open + end + 3);
            continue;
        }
        let Some(close) = html[open..].find('>').map(|found| open + found) else { break };
        let tag = &lower[open + 1..close];
        let name: String =
            tag.trim_start_matches('/').chars().take_while(|character| character.is_ascii_alphanumeric()).collect();
        at = close + 1;
        if !tag.starts_with('/') && UNREAD.contains(&name.as_str()) && !tag.ends_with('/') {
            // What it holds is skipped to its end.
            let end = format!("</{name}");
            at = lower[at..].find(&end).map_or(html.len(), |found| {
                let after = at + found;
                lower[after..].find('>').map_or(html.len(), |gt| after + gt + 1)
            });
            if name == "head" {
                // The title is the one thing in the head worth reading.
                if let Some(title) = title(&html[open..at], &lower[open..at]) {
                    out.push_str(&title);
                    out.push('\n');
                }
            }
            continue;
        }
        if BLOCKS.contains(&name.as_str()) {
            out.push('\n');
        }
    }
    if at < html.len() {
        out.push_str(&decoded(&html[at..]));
    }
    tidy(&out)
}

/// The text of the `<title>` within `head`, `lower` its lower case.
fn title(head: &str, lower: &str) -> Option<String> {
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    Some(decoded(&head[start..end]).trim().to_string())
}

/// `text` with its common entities decoded.
fn decoded(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // An entity ends within a few bytes of its `&`; looking no further
        // keeps a page of bare ampersands from being searched to its end
        // once for each.
        let Some(semi) = rest.bytes().take(11).position(|byte| byte == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let character = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "hellip" => Some('…'),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "middot" => Some('·'),
            "bull" => Some('•'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "trade" => Some('™'),
            "times" => Some('×'),
            "deg" => Some('°'),
            "equiv" => Some('≡'),
            "euro" => Some('€'),
            "yen" => Some('¥'),
            "pound" => Some('£'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|decimal| decimal.parse().ok()))
                .and_then(char::from_u32),
        };
        match character {
            Some(character) => {
                out.push(character);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` with each line's runs of blank space closed up and no more than
/// one empty line in a row.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut empty = 0;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            empty += 1;
            if empty > 1 || out.is_empty() {
                continue;
            }
        } else {
            empty = 0;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}
