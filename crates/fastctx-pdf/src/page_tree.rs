//! Reject page-tree damage that would silently change page numbering.

use std::collections::HashSet;

use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, Rect};

enum Step<'a> {
    Visit { node: Dict<'a>, has_media_box: bool },
    Count { before: usize, expected: usize },
}

pub(super) fn check(pdf: &Pdf) -> Result<(), String> {
    let xref = pdf.xref();
    let catalog = xref.get::<Dict<'_>>(xref.root_id()).ok_or("damaged PDF: missing catalog")?;
    let root = catalog.get::<Dict<'_>>(b"Pages".as_slice()).ok_or("damaged PDF: missing page tree")?;
    let mut pending = vec![Step::Visit { node: root, has_media_box: false }];
    let mut visited = HashSet::new();
    let mut count = 0;
    while let Some(step) = pending.pop() {
        let (node, inherited_media_box) = match step {
            Step::Visit { node, has_media_box } => (node, has_media_box),
            Step::Count { before, expected } => {
                if count - before != expected {
                    return Err("damaged PDF page tree: page count does not match its children".into());
                }
                continue;
            }
        };
        let has_media_box = if node.contains_key(b"MediaBox")
            && !matches!(node.get::<Object<'_>>(b"MediaBox".as_slice()), Some(Object::Null(_)))
        {
            node.get::<Rect>(b"MediaBox".as_slice()).ok_or("damaged PDF page tree: invalid MediaBox")?;
            true
        } else {
            inherited_media_box
        };
        if let Some(id) = node.obj_id()
            && !visited.insert(id)
        {
            return Err("damaged PDF page tree: cyclic or repeated page reference".into());
        }
        match node.get::<Name<'_>>(b"Type".as_slice()).as_deref() {
            Some(b"Pages") => {
                let expected =
                    node.get::<u32>(b"Count".as_slice()).ok_or("damaged PDF page tree: invalid page count")?;
                let kids =
                    node.get::<Array<'_>>(b"Kids".as_slice()).ok_or("damaged PDF page tree: missing children")?;
                pending.push(Step::Count { before: count, expected: expected as usize });
                let mut children = Vec::new();
                // ResolvedArrayIter ends on an unreadable reference. Raw
                // iteration preserves that distinction from the end of Kids.
                for kid in kids.raw_iter() {
                    let child = match kid {
                        MaybeRef::Ref(reference) => xref.get::<Dict<'_>>(reference.into()),
                        MaybeRef::NotRef(Object::Dict(dict)) => Some(dict),
                        _ => None,
                    };
                    children.push(child.ok_or("damaged PDF page tree: unreadable child")?);
                }
                pending.extend(children.into_iter().rev().map(|node| Step::Visit { node, has_media_box }));
            }
            Some(b"Page") => {
                // The engine supplies A4 when the required inherited MediaBox
                // is missing, and can reconstruct a broken tree by object ID.
                // Neither a guessed size nor a guessed page order is a view
                // of the selected page's actual contents.
                if !has_media_box {
                    return Err("damaged PDF page tree: missing MediaBox".into());
                }
                let page = pdf.pages().get(count).ok_or("damaged PDF page tree: unreadable page")?;
                if page.raw().obj_id() != node.obj_id() {
                    return Err("damaged PDF page tree: unreadable page".into());
                }
                count += 1;
            }
            _ => return Err("damaged PDF page tree: invalid node type".into()),
        }
    }
    if count != pdf.pages().len() {
        return Err("damaged PDF page tree: inconsistent page order".into());
    }
    Ok(())
}
