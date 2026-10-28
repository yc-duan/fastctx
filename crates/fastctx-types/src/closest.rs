//! The names most like one that names nothing, for an error a model reads:
//! a few to choose from, never the whole list (`world-concurrency.md`,
//! 给模型的永远是摘要).

/// The `most` of `names` nearest to `wanted`, by edit distance with case
/// ignored, nearer first and then by name.
pub fn names<'a>(wanted: &str, names: impl IntoIterator<Item = &'a str>, most: usize) -> Vec<&'a str> {
    let wanted: Vec<char> = wanted.to_lowercase().chars().collect();
    let mut scored: Vec<(usize, &str)> = names
        .into_iter()
        .map(|name| (distance(&wanted, &name.to_lowercase().chars().collect::<Vec<_>>()), name))
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(most).map(|(_, name)| name).collect()
}

/// How many characters to insert, delete or change to make `one` into `other`.
fn distance(one: &[char], other: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=other.len()).collect();
    for (i, a) in one.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in other.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(a != b));
            diagonal = above;
        }
    }
    row[other.len()]
}
