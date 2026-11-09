//! Coalesce only the edited neighborhood. Old snapshots keep their pages; a
//! missing sibling is Pending and a rejected plan publishes no partial tree.
use super::*;

impl<S: PageSource> Overlay<'_, S> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn coalesce(
        &mut self,
        children: &mut Vec<ManifestChild>,
        at: usize,
        changed: usize,
        kind: Kind,
        context: &TimelineRoot,
        meter: &mut Meter,
        depth: usize,
    ) -> Work<()> {
        if depth >= MAX_DEPTH {
            return Err(pages::corrupt("timeline tree exceeds its traversal depth").into());
        }
        let start = at.saturating_sub(1);
        let mut end = at.saturating_add(changed).saturating_add(1).min(children.len());
        let mut i = start;
        while i + 1 < end {
            if let Some(merged) = self.merge_pair(&children[i..i + 2], kind, context, meter, depth)? {
                children.splice(i..i + 2, [merged]);
                end -= 1;
                i = i.saturating_sub(1).max(start);
            } else {
                i += 1;
            }
        }
        Ok(())
    }
    fn merge_pair(
        &mut self,
        pair: &[ManifestChild],
        kind: Kind,
        context: &TimelineRoot,
        meter: &mut Meter,
        depth: usize,
    ) -> Work<Option<ManifestChild>> {
        let left = node(self, &pair[0], kind, context, meter)?;
        let right = node(self, &pair[1], kind, context, meter)?;
        let page = match (left, right) {
            (TreeNode::Leaf(mut entries), TreeNode::Leaf(right)) => {
                entries.extend(right);
                packing::leaf_page(kind, context, &entries)?
            }
            (TreeNode::Branch(mut children), TreeNode::Branch(right)) => {
                let seam = children.len();
                children.extend(right);
                // A seam merge can remove one child range before the parent
                // count is checked, including the 128 + 129 boundary case.
                if children.len() > 257 {
                    None
                } else {
                    // Joining parents also creates a new sibling boundary one
                    // level down. Compact that seam instead of stranding sparse
                    // leaves behind their former parents.
                    self.coalesce(&mut children, seam, 0, kind, context, meter, depth + 1)?;
                    if children.len() > 256 {
                        None
                    } else {
                        pages::encode_branch(&context.conversation.scope, children)?
                    }
                }
            }
            // The wire shape does not require uniform leaf depth. A valid old
            // mixed-depth tree is readable but these two nodes cannot coalesce.
            _ => None,
        };
        let Some(page) = page else { return Ok(None) };
        Ok(Some(ManifestChild {
            first_key: pair[0].first_key.clone(),
            last_key: pair[1].last_key.clone(),
            root: self.add(page, meter)?,
            entries: Count(total(pair)?),
        }))
    }
    pub(super) fn collapse(
        &self,
        mut child: ManifestChild,
        kind: Kind,
        context: &TimelineRoot,
        meter: &mut Meter,
    ) -> Work<ManifestChild> {
        for _ in 0..MAX_DEPTH {
            match node(self, &child, kind, context, meter)? {
                TreeNode::Branch(mut children) if children.len() == 1 => {
                    child = children.pop().expect("one child");
                }
                _ => return Ok(child),
            }
        }
        Err(pages::corrupt("timeline tree exceeds its traversal depth").into())
    }
}
