//! The World's events the merge put among the host's (`super::Layout`), in
//! the merged order (design §28.51 ⑥-2): a tree of their places by
//! position, each node knowing how many it holds, the gap of the last of
//! them, and the latest of their times — so that where one stands,
//! the first later than a time, and where a new one goes are each found in
//! about the logarithm of their number, not by reading them all.

use super::super::Moment;

/// No node.
const NONE: u32 = u32::MAX;

/// The most a node holds before it is parted in two.
const WIDE: usize = 64;

struct Node<T> {
    parent: u32,
    /// A leaf's puts, or an inner node's children, in their order.
    items: Vec<u32>,
    leaf: bool,
    /// How many puts are under it.
    size: u32,
    /// The latest time under it; `Moment::NONE` for none.
    latest: T,
    /// The gap of the last put under it.
    last_gap: u32,
}

impl<T: Moment> Node<T> {
    /// Room for as many as it may hold before it is parted, none more: a
    /// node's items never grow past it.
    fn new(parent: u32, items: Vec<u32>, leaf: bool) -> Self {
        let mut held = Vec::with_capacity(WIDE + 1);
        held.extend(items);
        Node { parent, items: held, leaf, size: 0, latest: T::NONE, last_gap: 0 }
    }
}

/// The puts, each known by its number among the World's events, in the
/// merged order: by gap (the host's event they stand before), and within a
/// gap as they were put.
pub struct Puts<T> {
    nodes: Vec<Node<T>>,
    root: u32,
    /// By each World event's number: the gap it was put in and the leaf it
    /// stands in, for those put; and its time (of what stands in its place
    /// now, for those put).
    gap: Vec<u32>,
    leaf: Vec<u32>,
    moment: Vec<T>,
}

impl<T: Moment> Puts<T> {
    /// None put yet, of World events at `moments`.
    pub fn new(moments: Vec<T>) -> Self {
        let events = moments.len();
        Puts { nodes: vec![Node::new(NONE, Vec::new(), true)], root: 0, gap: vec![NONE; events], leaf: vec![NONE; events], moment: moments }
    }

    /// The time of the World's event `event`, or of what stands in its place
    /// now where it was put.
    pub fn moment(&self, event: usize) -> T {
        self.moment[event]
    }

    /// The gap `put` stands in.
    pub fn gap(&self, put: usize) -> usize {
        self.gap[put] as usize
    }

    /// `put` after every put of gap `gap` or less, before those of a later gap.
    pub fn insert_in_gap(&mut self, put: usize, gap: usize) {
        let gap = u32::try_from(gap).expect("fewer events than numbers");
        let mut node = self.root;
        while !self.nodes[node as usize].leaf {
            let items = &self.nodes[node as usize].items;
            node = items.iter().copied().find(|child| self.nodes[*child as usize].last_gap > gap).unwrap_or(*items.last().expect("an inner node has children"));
        }
        let items = &self.nodes[node as usize].items;
        let position = items.iter().position(|other| self.gap[*other as usize] > gap).unwrap_or(items.len());
        // At the start of a leaf, the end of the one before it is the same
        // place: there, as a leaf grown at its end stays full.
        let parent = self.nodes[node as usize].parent;
        if position == 0 && parent != NONE {
            let siblings = &self.nodes[parent as usize].items;
            let at = siblings.iter().position(|child| *child == node).expect("a child in its parent");
            if at > 0 {
                let before = siblings[at - 1];
                let end = self.nodes[before as usize].items.len();
                return self.place(before, end, put, gap);
            }
        }
        self.place(node, position, put, gap);
    }

    /// `put` right before `before`, in its gap.
    pub fn insert_before(&mut self, put: usize, before: usize) {
        let (gap, node) = (self.gap[before], self.leaf[before]);
        let position = self.position(node, before);
        self.place(node, position, put, gap);
    }

    /// `put` after all, in gap `gap`.
    pub fn push(&mut self, put: usize, gap: usize) {
        let gap = u32::try_from(gap).expect("fewer events than numbers");
        let mut node = self.root;
        while !self.nodes[node as usize].leaf {
            node = *self.nodes[node as usize].items.last().expect("an inner node has children");
        }
        let position = self.nodes[node as usize].items.len();
        self.place(node, position, put, gap);
    }

    fn position(&self, node: u32, put: usize) -> usize {
        self.nodes[node as usize].items.iter().position(|other| *other as usize == put).expect("a put in its leaf")
    }

    fn place(&mut self, leaf: u32, position: usize, put: usize, gap: u32) {
        self.gap[put] = gap;
        self.leaf[put] = leaf;
        let moment = self.moment[put];
        self.nodes[leaf as usize].items.insert(position, u32::try_from(put).expect("fewer events than numbers"));
        // One more under each node up from the leaf; its latest and the gap
        // of its last as they are now.
        let mut node = leaf;
        while node != NONE {
            let last_gap = self.last_gap_of(node);
            let held = &mut self.nodes[node as usize];
            held.size += 1;
            held.latest = held.latest.max(moment);
            held.last_gap = last_gap;
            node = held.parent;
        }
        if self.nodes[leaf as usize].items.len() > WIDE {
            self.split(leaf, position == WIDE);
        }
    }

    /// The gap of the last put under `node`, as its items stand.
    fn last_gap_of(&self, node: u32) -> u32 {
        let held = &self.nodes[node as usize];
        match held.items.last() {
            None => 0,
            Some(last) if held.leaf => self.gap[*last as usize],
            Some(last) => self.nodes[*last as usize].last_gap,
        }
    }

    /// What `node` knows of what is under it, made again from its items.
    fn refresh(&mut self, node: u32) {
        let held = &self.nodes[node as usize];
        let (size, latest) = if held.leaf {
            (held.items.len() as u32, held.items.iter().map(|put| self.moment[*put as usize]).max().unwrap_or(T::NONE))
        } else {
            held.items.iter().fold((0, T::NONE), |(size, latest), child| {
                let child = &self.nodes[*child as usize];
                (size + child.size, latest.max(child.latest))
            })
        };
        let last_gap = self.last_gap_of(node);
        let held = &mut self.nodes[node as usize];
        (held.size, held.latest, held.last_gap) = (size, latest, last_gap);
    }

    /// `node`, too full, parted in two; its parent too, as far as it must.
    /// Grown at its end (`at_end`) — as puts mostly come, each later than
    /// those before — it keeps all but the last, so that nodes stay full;
    /// else it is parted in halves.
    fn split(&mut self, node: u32, at_end: bool) {
        let (mut node, mut at_end) = (node, at_end);
        while self.nodes[node as usize].items.len() > WIDE {
            let half = if at_end { WIDE } else { self.nodes[node as usize].items.len() / 2 };
            let moved: Vec<u32> = self.nodes[node as usize].items.drain(half..).collect();
            let leaf = self.nodes[node as usize].leaf;
            let parent = self.nodes[node as usize].parent;
            let new = u32::try_from(self.nodes.len()).expect("fewer nodes than numbers");
            for item in &moved {
                if leaf {
                    self.leaf[*item as usize] = new;
                } else {
                    self.nodes[*item as usize].parent = new;
                }
            }
            self.nodes.push(Node::new(parent, moved, leaf));
            self.refresh(node);
            self.refresh(new);
            if parent == NONE {
                // A new root over the two: what is under it is as it was.
                let root = new + 1;
                self.nodes.push(Node::new(NONE, vec![node, new], false));
                self.nodes[node as usize].parent = root;
                self.nodes[new as usize].parent = root;
                self.refresh(root);
                self.root = root;
                return;
            }
            let at = self.nodes[parent as usize].items.iter().position(|child| *child == node).expect("a child in its parent");
            self.nodes[parent as usize].items.insert(at + 1, new);
            at_end = at + 1 == WIDE;
            node = parent;
        }
    }

    /// Where `put` stands among all the puts.
    fn index(&self, put: usize) -> usize {
        let mut node = self.leaf[put];
        let mut index = self.position(node, put);
        loop {
            let parent = self.nodes[node as usize].parent;
            if parent == NONE {
                return index;
            }
            for child in &self.nodes[parent as usize].items {
                if *child == node {
                    break;
                }
                index += self.nodes[*child as usize].size as usize;
            }
            node = parent;
        }
    }

    /// Whether `one` stands before `other` in the merged order.
    pub fn before(&self, one: usize, other: usize) -> bool {
        match self.gap[one].cmp(&self.gap[other]) {
            std::cmp::Ordering::Equal => self.index(one) < self.index(other),
            order => order.is_lt(),
        }
    }

    /// Of `puts`, the first in the merged order: each placed once.
    pub fn first_of(&self, puts: impl IntoIterator<Item = usize>) -> Option<usize> {
        puts.into_iter().map(|put| ((self.gap[put], self.index(put)), put)).min().map(|(_, put)| put)
    }

    /// The first put after `after` (from the first, with none) whose time
    /// is later than `at`.
    pub fn first_later(&self, after: Option<usize>, at: T) -> Option<usize> {
        let from = after.map_or(0, |after| self.index(after) + 1);
        self.find(self.root, 0, from, at)
    }

    fn find(&self, node: u32, offset: usize, from: usize, at: T) -> Option<usize> {
        let held = &self.nodes[node as usize];
        if held.latest <= at || offset + held.size as usize <= from {
            return None;
        }
        if held.leaf {
            return held.items.iter().enumerate().find(|(index, put)| offset + index >= from && self.moment[**put as usize] > at).map(|(_, put)| *put as usize);
        }
        let mut offset = offset;
        for child in &held.items {
            if let Some(found) = self.find(*child, offset, from, at) {
                return Some(found);
            }
            offset += self.nodes[*child as usize].size as usize;
        }
        None
    }

    /// What stands in `put`'s place now is at `at`.
    pub fn set_moment(&mut self, put: usize, at: T) {
        self.moment[put] = at;
        let mut node = self.leaf[put];
        while node != NONE {
            self.refresh(node);
            node = self.nodes[node as usize].parent;
        }
    }

    /// The puts in the merged order, each with its gap.
    pub fn into_order(self) -> impl Iterator<Item = (usize, usize)> {
        // Down the tree from its root, each node's items in turn.
        let mut stack: Vec<(u32, usize)> = vec![(self.root, 0)];
        std::iter::from_fn(move || {
            loop {
                let (node, next) = stack.last_mut()?;
                let held = &self.nodes[*node as usize];
                let Some(item) = held.items.get(*next).copied() else {
                    stack.pop();
                    continue;
                };
                *next += 1;
                if held.leaf {
                    return Some((item as usize, self.gap[item as usize] as usize));
                }
                stack.push((item, 0));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Choices(u64);
    impl Choices {
        fn pick(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// The puts as a list kept by `Vec::insert`, as the merge kept them,
    /// against the tree: where each goes, which stands first, the first
    /// later than a time, and their order at the end.
    #[test]
    fn the_tree_of_puts_is_the_list_of_puts() {
        let mut choices = Choices(0x1234_5678_9abc_def1);
        for round in 0..300 {
            let events = 1 + choices.pick(if round % 30 == 0 { 3_000 } else { 200 }) as usize;
            let spread = 1 + choices.pick(40);
            let ranks: Vec<i64> = (0..events).map(|_| 1 + choices.pick(spread) as i64).collect();
            let gaps = 1 + choices.pick(20) as usize;
            let mut tree = Puts::new(ranks.clone());
            let mut ranks = ranks;
            // (event, gap), in the merged order.
            let mut list: Vec<(usize, usize)> = Vec::new();
            for event in 0..events {
                match choices.pick(3) {
                    0 if !list.is_empty() => {
                        let index = choices.pick(list.len() as u64) as usize;
                        let (before, gap) = list[index];
                        list.insert(index, (event, gap));
                        tree.insert_before(event, before);
                    }
                    1 => {
                        list.push((event, gaps));
                        tree.push(event, gaps);
                    }
                    _ => {
                        let gap = choices.pick(gaps as u64) as usize;
                        let index = list.partition_point(|(_, other)| *other <= gap);
                        list.insert(index, (event, gap));
                        tree.insert_in_gap(event, gap);
                    }
                }
                if choices.pick(5) == 0 && !list.is_empty() {
                    let (put, _) = list[choices.pick(list.len() as u64) as usize];
                    let rank = 1 + choices.pick(spread) as i64;
                    ranks[put] = rank;
                    tree.set_moment(put, rank);
                }
                let rank = choices.pick(spread + 1) as i64;
                let after = (!list.is_empty() && choices.pick(2) == 0).then(|| list[choices.pick(list.len() as u64) as usize].0);
                let from = after.map_or(0, |after| list.iter().position(|(put, _)| *put == after).expect("listed") + 1);
                let was = list[from..].iter().find(|(put, _)| ranks[*put] > rank).map(|(put, _)| *put);
                assert_eq!(tree.first_later(after, rank), was, "round {round} event {event}");
                if list.len() >= 2 {
                    let (one, other) = (choices.pick(list.len() as u64) as usize, choices.pick(list.len() as u64) as usize);
                    assert_eq!(tree.before(list[one].0, list[other].0), one < other, "round {round}");
                    assert_eq!(tree.gap(list[one].0), list[one].1);
                }
            }
            assert_eq!(tree.into_order().collect::<Vec<_>>(), list, "round {round}");
        }
    }
}
