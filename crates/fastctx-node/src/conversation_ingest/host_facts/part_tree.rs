//! Binary carries keep public part trees logarithmic in both scan directions.
use super::content;
use crate::conversation_ingest::step_content::StepContent;
use fastctx_sync::store::Error;
use fastctx_types::sync::{conversation_timeline::*, cursor::Count, manifest::ContentRef};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Tree {
    height: u8,
    range: DisplayPartsRef,
}
/// A suspended builder retains only completed leaves and a logarithmic forest.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct State {
    kind: Option<FactPartKindV1>,
    reverse: bool,
    forest: Vec<Tree>,
    last: Option<Count>,
}
pub struct Builder<'a, 'source> {
    io: &'a StepContent<'source>,
    kind: Option<FactPartKindV1>,
    reverse: bool,
    parts: Vec<(Count, FactPartV1)>,
    forest: Vec<Tree>,
    last: Option<Count>,
}
impl<'a, 'source> Builder<'a, 'source> {
    pub fn new(io: &'a StepContent<'source>, kind: Option<FactPartKindV1>, reverse: bool) -> Self {
        Self { io, kind, reverse, parts: vec![], forest: vec![], last: None }
    }
    pub fn resume(io: &'a StepContent<'source>, state: State) -> Self {
        Self { io, kind: state.kind, reverse: state.reverse, parts: vec![], forest: state.forest, last: state.last }
    }
    pub async fn checkpoint(mut self) -> Result<State, Error> {
        self.flush().await?;
        Ok(State { kind: self.kind, reverse: self.reverse, forest: self.forest, last: self.last })
    }
    pub async fn push(&mut self, index: Count, part: FactPartV1) -> Result<(), Error> {
        if let Some(last) = self.last {
            let consecutive = if self.reverse {
                index.0.checked_add(1) == Some(last.0)
            } else {
                last.0.checked_add(1) == Some(index.0)
            };
            if !consecutive {
                return Err(Error::Invalid("facts parts are not consecutive".into()));
            }
        }
        if self.kind.is_some_and(|kind| kind != part.kind()) {
            return Err(Error::Invalid("facts list mixes part kinds".into()));
        }
        self.kind = Some(part.kind());
        self.parts.push((index, part));
        if self.leaf()?.encode().is_err() {
            let last = self.parts.pop().ok_or(Error::Busy)?;
            self.flush().await?;
            self.parts.push(last);
        }
        self.last = Some(index);
        Ok(())
    }
    fn leaf(&self) -> Result<ItemFactBodyV1, Error> {
        let start = if self.reverse { self.parts.last() } else { self.parts.first() }
            .map(|(index, _)| *index)
            .ok_or(Error::Busy)?;
        let mut parts: Vec<_> = self.parts.iter().map(|(_, part)| part.clone()).collect();
        if self.reverse {
            parts.reverse();
        }
        Ok(ItemFactBodyV1 {
            version: FACTS_VERSION,
            fact: ItemFactV1::Parts { part_kind: self.kind.ok_or(Error::Busy)?, start, parts },
        })
    }
    async fn flush(&mut self) -> Result<(), Error> {
        if self.parts.is_empty() {
            return Ok(());
        }
        let leaf = self.leaf()?;
        let ItemFactV1::Parts { start, ref parts, .. } = leaf.fact else { return Err(Error::Busy) };
        let count = Count(parts.len() as u64);
        let mut tree =
            Tree { height: 0, range: descriptor(start, count, content::publish(self.io, leaf.fact).await?)? };
        self.parts.clear();
        while self.forest.last().is_some_and(|previous| previous.height == tree.height) {
            let previous = self.forest.pop().ok_or(Error::Busy)?;
            tree = if self.reverse { self.join(tree, previous).await? } else { self.join(previous, tree).await? };
        }
        self.forest.push(tree);
        Ok(())
    }
    async fn join(&self, left: Tree, right: Tree) -> Result<Tree, Error> {
        if left.range.start.0.checked_add(left.range.count.0) != Some(right.range.start.0) {
            return Err(Error::Invalid("facts tree leaves a range gap".into()));
        }
        let height = left.height.max(right.height).checked_add(1).filter(|height| *height <= 64).ok_or(Error::Busy)?;
        let start = left.range.start;
        let count = Count(left.range.count.0.checked_add(right.range.count.0).ok_or(Error::Busy)?);
        let branch = ItemDisplayPartsBranchV1 {
            version: DISPLAY_VERSION,
            start,
            count,
            children: vec![left.range, right.range],
        };
        let root = match content::part_branch(self.io, &branch).await? {
            ContentRef::Manifest { root } => root,
            _ => return Err(Error::Busy),
        };
        Ok(Tree { height, range: DisplayPartsRef { start, count, root } })
    }
    pub async fn finish(mut self) -> Result<Option<ContentRef>, Error> {
        self.flush().await?;
        let Some(mut tree) = self.forest.pop() else { return Ok(None) };
        while let Some(previous) = self.forest.pop() {
            tree = if self.reverse { self.join(tree, previous).await? } else { self.join(previous, tree).await? };
        }
        Ok(Some(ContentRef::Manifest { root: tree.range.root }))
    }
}

fn descriptor(start: Count, count: Count, content: ContentRef) -> Result<DisplayPartsRef, Error> {
    let ContentRef::Manifest { root } = content else {
        return Err(Error::Invalid("facts leaf is not a typed descriptor".into()));
    };
    Ok(DisplayPartsRef { start, count, root })
}
