//! The reviewer receives a bounded projection; complete cause-scoped data stays readable.

use super::{super::reviewer::Materials, failure};
use fastctx_types::ipc::Failure;

pub(super) const MAX_BYTES: usize = 24 * 1024;
const EXCERPT: &str = "\n[excerpt; see full_path]\n";

fn excerpt(text: &str, bytes: usize, cut: &mut bool) -> String {
    if text.len() <= bytes {
        return text.into();
    }
    *cut = true;
    let side = bytes.saturating_sub(EXCERPT.len()) / 2;
    let mut head = side.min(text.len());
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len().saturating_sub(side);
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{EXCERPT}{}", &text[..head], &text[tail..])
}

fn lines(text: &str, bytes: usize, cut: &mut bool) -> String {
    if text.len() <= bytes {
        return text.into();
    }
    *cut = true;
    let mut selected = String::new();
    for line in text.lines() {
        if selected.len() + line.len() + 1 + EXCERPT.len() > bytes {
            break;
        }
        selected.push_str(line);
        selected.push('\n');
    }
    selected.push_str(EXCERPT);
    selected
}

fn project(full: &Materials, scale: usize) -> Materials {
    let mut result = full.clone();
    let mut cut = full.truncated;
    let budget = |bytes: usize| (bytes / scale).max(64);
    result.objective = excerpt(&full.objective, budget(4096), &mut cut);
    result.plan_claim = full.plan_claim.as_ref().map(|text| excerpt(text, budget(3072), &mut cut));
    result.blocked_claim = full.blocked_claim.as_ref().map(|text| excerpt(text, budget(3072), &mut cut));
    result.last_words_claim = full.last_words_claim.as_ref().map(|text| excerpt(text, budget(1200), &mut cut));
    if let Some(facts) = &mut result.world_actions {
        facts.summary = lines(&facts.summary, budget(6144), &mut cut);
    }
    if let Some(stop) = &mut result.stopped {
        stop.message = stop.message.as_ref().map(|text| excerpt(text, budget(1024), &mut cut));
        if stop.category.as_ref().is_some_and(|value| value.to_string().len() > budget(512)) {
            stop.category = None;
            stop.message =
                Some(format!("{}\n[Host category omitted; see full_path]", stop.message.take().unwrap_or_default()));
            cut = true;
        }
    }
    for words in &mut result.owner_words {
        words.text = excerpt(&words.text, budget(1024), &mut cut);
    }
    cut |= result.previous_judgments.len() > 3;
    result.previous_judgments =
        full.previous_judgments.iter().rev().take(3).map(|text| excerpt(text, budget(1024), &mut cut)).collect();
    result.previous_judgments.reverse();
    // This is a display catalog, not authority: access is determined by the
    // account's current directory. The complete catalog remains in full_path.
    if result.machines.len() > 16 {
        result.machines_total = Some(result.machines.len());
        result.machines.truncate(16);
        cut = true;
    }
    result.truncated = cut;
    result
}

/// Called before the request is submitted. The frozen file contains every
/// eligible field, including claims that cannot fit in the initial prompt.
pub(super) fn freeze(full: Materials) -> Result<Materials, Failure> {
    for scale in [1, 2, 4, 8, 16, 32, 64] {
        let projected = project(&full, scale);
        if serde_json::to_vec(&projected).map_err(failure)?.len() <= MAX_BYTES {
            return Ok(projected);
        }
    }
    Err(Failure::new("the goal review's identifiers and source paths exceed its material budget"))
}

/// Old frozen requests have no comparison snapshot. Their original data can
/// be bounded without claiming newly read observations were already reviewed.
pub(super) fn cached(materials: Materials) -> Result<Materials, Failure> {
    if materials.full_path.is_some() && serde_json::to_vec(&materials).map_err(failure)?.len() <= MAX_BYTES {
        return Ok(materials);
    }
    freeze(materials)
}
