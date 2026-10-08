//! Preserve independently aligned paragraphs when their longest lines nearly touch.

use hayro::kurbo::Rect;

pub(super) struct Fragment {
    pub bounds: Rect,
    pub baseline: f64,
    pub em: f64,
    pub text: String,
}

pub(super) struct Columns {
    pub membership: Vec<Option<usize>>,
    bounds: Vec<Rect>,
}

impl Columns {
    pub fn find(fragments: &[Fragment]) -> Self {
        let mut result = Self { membership: vec![None; fragments.len()], bounds: Vec::new() };
        let indices: Vec<_> = fragments
            .iter()
            .enumerate()
            .filter(|(_, part)| !part.text.trim().is_empty())
            .map(|(index, _)| index)
            .collect();
        let body = |part: &Fragment| part.text.chars().any(char::is_alphabetic) && !label(&part.text);
        let mut start = 0;
        while start < indices.len() {
            let mut end = start + 1;
            while end < indices.len() {
                let a = &fragments[indices[end - 1]];
                let b = &fragments[indices[end]];
                let em = a.em.min(b.em);
                let step = b.baseline - a.baseline;
                // Repeated alignment and consecutive line steps are evidence
                // of a paragraph. Two font heights allow leading but exclude
                // an intervening empty line. Number columns are not prose.
                if !body(a)
                    || !body(b)
                    || [
                        (a.bounds.x0 - b.bounds.x0).abs(),
                        (a.bounds.x1 - b.bounds.x1).abs(),
                        (a.bounds.center().x - b.bounds.center().x).abs(),
                    ]
                    .into_iter()
                    .fold(f64::INFINITY, f64::min)
                        > 0.15 * em
                    || step < 0.8 * em
                    || step > a.em + b.em
                {
                    break;
                }
                end += 1;
            }
            if end - start >= 2 {
                let id = result.bounds.len();
                let mut bounds = fragments[indices[start]].bounds;
                for &index in &indices[start..end] {
                    result.membership[index] = Some(id);
                    bounds = bounds.union(fragments[index].bounds);
                }
                result.bounds.push(bounds);
            }
            start = end;
        }
        result.numbered_columns(fragments);
        result
    }

    fn numbered_columns(&mut self, fragments: &[Fragment]) {
        let mut markers: Vec<_> = fragments.iter().filter(|part| label(&part.text)).collect();
        markers.sort_by(|a, b| a.bounds.x0.total_cmp(&b.bounds.x0));
        let mut axes: Vec<Vec<&Fragment>> = Vec::new();
        for marker in markers {
            if let Some(axis) = axes.last_mut()
                && (marker.bounds.x0 - axis[0].bounds.x0).abs() < 0.15 * marker.em
            {
                axis.push(marker);
            } else {
                axes.push(vec![marker]);
            }
        }
        axes.retain(|axis| axis.len() >= 2);
        if axes.len() < 2 {
            return;
        }
        // Repeated numbering at distinct left edges is direct evidence of
        // independent lists. Their descriptions may approach the next list
        // more closely than their own hanging indent approaches the number.
        for (axis_index, axis) in axes.iter().enumerate() {
            let left = axis[0].bounds.x0;
            let right = axes.get(axis_index + 1).map_or(f64::INFINITY, |axis| axis[0].bounds.x0);
            let top = axis.iter().map(|part| part.bounds.y0).fold(f64::INFINITY, f64::min);
            let em = axis[0].em;
            let mut bottom = axis.iter().map(|part| part.baseline).fold(f64::NEG_INFINITY, f64::max);
            let indent = axis.iter().map(|part| part.bounds.x1).fold(f64::NEG_INFINITY, f64::max);
            let mut following: Vec<_> = fragments
                .iter()
                .filter(|part| {
                    part.bounds.x0 >= indent
                        && part.bounds.x0 < right
                        && part.baseline >= bottom
                        && part.text.chars().any(char::is_alphabetic)
                })
                .collect();
            following.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
            for part in following {
                if part.baseline - bottom > em + part.em {
                    break;
                }
                bottom = bottom.max(part.baseline);
            }
            let id = self.bounds.len();
            let mut actual_right = left;
            for (index, part) in fragments.iter().enumerate() {
                if part.bounds.x0 >= left - 0.15 * em
                    && part.bounds.x0 < right - 0.15 * em
                    && part.baseline >= top
                    && part.baseline <= bottom + 0.15 * em
                {
                    self.membership[index] = Some(id);
                    actual_right = actual_right.max(part.bounds.x1);
                }
            }
            self.bounds.push(Rect::new(left, top, actual_right.min(right), bottom + em));
        }
    }

    pub fn separates(&self, first: Option<usize>, second: Option<usize>) -> bool {
        let (Some(a), Some(b)) = (first, second) else { return false };
        if a == b {
            return false;
        }
        let (a, b) = (self.bounds[a], self.bounds[b]);
        a.y0 < b.y1 && b.y0 < a.y1 && (a.x1 <= b.x0 || b.x1 <= a.x0)
    }
}

pub(super) fn label(text: &str) -> bool {
    let text = text.trim();

    let Some(number) = text.strip_suffix(')').or_else(|| text.strip_suffix('.')).or_else(|| text.strip_suffix('、'))
    else {
        return false;
    };
    let number = number.strip_prefix('(').unwrap_or(number);
    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
}
