use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::io;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use fastctx_types::file_search::FileMatch;
use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher};

use crate::{check_open, index::Index, paths, rules};

pub(crate) fn query(
    index: &Index,
    query: &str,
    limit: usize,
    closed: &AtomicBool,
) -> io::Result<Option<(usize, Vec<FileMatch>)>> {
    check_open(closed)?;
    if rules::changed(&index.outer_rules)? {
        return Ok(None);
    }
    let pattern_text = if cfg!(windows) { query.replace('\\', "/") } else { query.to_owned() };
    let atom = Atom::new(&pattern_text, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy, false);
    let lower = pattern_text.to_lowercase();
    let path_query = pattern_text.contains('/');
    let (total, best) = if index.entries.is_empty() || std::thread::available_parallelism()?.get() == 1 {
        ranked(index, &atom, &lower, path_query, 0..index.entries.len(), limit, closed)?
    } else {
        // One helper shares the scoring pass without a machine-wide pool:
        // the node also runs residents and other concurrent pickers.
        std::thread::scope(|scope| -> io::Result<_> {
            let middle = index.entries.len() / 2;
            let (needle, lowercase) = (&atom, lower.as_str());
            let helper =
                std::thread::Builder::new().name("file-search-match".into()).spawn_scoped(scope, move || {
                    ranked(index, needle, lowercase, path_query, middle..index.entries.len(), limit, closed)
                })?;
            let result = ranked(index, &atom, &lower, path_query, 0..middle, limit, closed);
            let other = helper.join().map_err(|_| io::Error::other("file matching worker panicked"))?;
            let (mut total, mut best) = result?;
            let (other_total, other_best) = other?;
            total += other_total;
            for Reverse(rank) in other_best {
                retain(&mut best, rank, limit);
            }
            Ok((total, best))
        })?
    };
    check_open(closed)?;
    let mut ordered = best.into_vec();
    ordered.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut files = Vec::with_capacity(ordered.len());
    let mut checked = HashSet::new();
    let mut ancestors = HashSet::new();
    for Reverse((_, _, _, _, _, position)) in ordered {
        check_open(closed)?;
        let entry = &index.entries[position];
        let root = &index.roots[entry.root];
        let path = index.path(entry);
        if !paths::contained_file(&path, root, &mut ancestors)? || index.policy_changed(&path, &mut checked)? {
            return Ok(None);
        }
        files.push(FileMatch { path: paths::text(&path)?, root: paths::text(root)?, relative: entry.relative.clone() });
    }
    if rules::changed(&index.outer_rules)? {
        return Ok(None);
    }
    check_open(closed)?;
    Ok(Some((total, files)))
}

type Rank<'a> = (u8, u16, Reverse<usize>, Reverse<&'a str>, Reverse<&'a Path>, usize);

fn ranked<'a>(
    index: &'a Index,
    atom: &Atom,
    lower: &str,
    path_query: bool,
    range: Range<usize>,
    limit: usize,
    closed: &AtomicBool,
) -> io::Result<(usize, BinaryHeap<Reverse<Rank<'a>>>)> {
    check_open(closed)?;
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut best = BinaryHeap::new();
    let mut total = 0;
    let mut name_scores = vec![None; index.names.len()];
    for position in range {
        check_open(closed)?;
        let entry = &index.entries[position];
        let name = &index.names[entry.name];
        let name_score = if path_query {
            None
        } else {
            *name_scores[entry.name].get_or_insert_with(|| atom.score(name.text.slice(..), &mut matcher))
        };
        let Some(score) = name_score.or_else(|| atom.score(entry.text.slice(..), &mut matcher)) else { continue };
        total += 1;
        if limit == 0 {
            continue;
        }
        let priority = if lower.is_empty() {
            0
        } else if name.lower == lower {
            3
        } else if name.lower.starts_with(lower) {
            2
        } else if name_score.is_some() {
            1
        } else {
            0
        };
        // Heap root is the worst retained hit. Ties use a stable path order,
        // never directory enumeration order or matcher injection order.
        let rank = (
            priority,
            score,
            Reverse(entry.relative.len()),
            Reverse(entry.relative.as_str()),
            Reverse(index.roots[entry.root].as_path()),
            position,
        );
        retain(&mut best, rank, limit);
    }
    Ok((total, best))
}

fn retain<'a>(best: &mut BinaryHeap<Reverse<Rank<'a>>>, rank: Rank<'a>, limit: usize) {
    if best.len() < limit {
        best.push(Reverse(rank));
    } else if best.peek().is_some_and(|worst| rank > worst.0) {
        *best.peek_mut().expect("nonempty heap") = Reverse(rank);
    }
}
