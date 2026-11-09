//! Reflexes evaluate bounded observations without invoking a model: a look
//! or a page repeated on a schedule, tested by a predicate, or a time
//! (`world-senses.md`, 反射). None runs a program.
use crate::ports::{Look, ReadResult, Web};
use fastctx_agent::runtime::bounded;
use fastctx_types::sentinel::*;

/// The shortest interval of a reflex that looks at a machine.
pub const LOOK_EVERY_MS: u64 = 30_000;
/// The shortest interval of a reflex that reads a page.
pub const PAGE_EVERY_MS: u64 = 300_000;
/// The shortest interval of a reflex that only wakes its sentinel, each
/// wake a call to its model.
pub const WAKE_EVERY_MS: u64 = 300_000;

#[derive(Debug)]
pub enum Evaluation {
    Quiet,
    Wake(WakeReason),
}

pub fn validate(reflex: &Reflex, kind: Kind, now: i64) -> Result<(), String> {
    if kind == Kind::Goal {
        return Err("goal sentinels cannot install reflexes".into());
    }
    if reflex.id.is_empty()
        || reflex.id.len() > 64
        || !reflex.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("invalid reflex ID".into());
    }
    if reflex.at <= now {
        return Err("reflex must wake in the future".into());
    }
    let shortest = match (&reflex.probe, kind) {
        (None, _) => WAKE_EVERY_MS,
        (Some(Probe::Look { inspect, .. }), Kind::Local) => {
            inspect.check()?;
            LOOK_EVERY_MS
        }
        (Some(Probe::Web { url }), Kind::Web) if url.len() <= 4096 => PAGE_EVERY_MS,
        _ => return Err("reflex probe is not available to this sentinel".into()),
    };
    if reflex.every_ms.is_some_and(|every| !(shortest..=31_536_000_000).contains(&every)) {
        return Err(format!("this reflex repeats at most every {} seconds", shortest / 1000));
    }
    match (&reflex.probe, &reflex.predicate) {
        (None, None) | (Some(_), Some(_)) => {}
        (Some(_), None) => return Err("a reflex that looks needs a predicate".into()),
        (None, Some(_)) => return Err("a predicate needs a probe to test".into()),
    }
    match &reflex.predicate {
        Some(Predicate::Contains { text } | Predicate::Absent { text }) if text.is_empty() || text.len() > 4096 => {
            Err("invalid reflex text".into())
        }
        Some(Predicate::Number { prefix, value, .. })
            if prefix.is_empty() || prefix.len() > 4096 || !value.is_finite() =>
        {
            Err("invalid numeric predicate".into())
        }
        _ => Ok(()),
    }
}

/// Caller persists the updated baseline and next time together with any wake intent.
/// Missed periods coalesce into one probe; there is no catch-up burst after reboot.
pub async fn evaluate(
    definition: &Definition,
    reflex: &mut Reflex,
    now: i64,
    look: &dyn Look,
    web: &dyn Web,
) -> Evaluation {
    match tokio::time::timeout(std::time::Duration::from_secs(60), evaluate_inner(definition, reflex, now, look, web))
        .await
    {
        Ok(result) => result,
        Err(_) => Evaluation::Wake(WakeReason::Broken {
            id: reflex.id.clone(),
            reason: "reflex timed out after 60 seconds".into(),
        }),
    }
}
async fn evaluate_inner(
    definition: &Definition,
    reflex: &mut Reflex,
    now: i64,
    look: &dyn Look,
    web: &dyn Web,
) -> Evaluation {
    if reflex.at > now {
        return Evaluation::Quiet;
    }
    let due = reflex.at;
    reflex.at = reflex.every_ms.map_or(i64::MAX, |period| now.saturating_add(period as i64));
    let result = match &reflex.probe {
        None => return Evaluation::Wake(WakeReason::Time { id: reflex.id.clone(), due }),
        Some(Probe::Look { machine, inspect }) if definition.kind == Kind::Local => {
            look.look(definition, machine, inspect).await
        }
        Some(Probe::Web { url }) if definition.kind == Kind::Web => web.fetch(url).await,
        _ => Err("reflex capability does not match sentinel kind".into()),
    };
    let result = result.and_then(|result| {
        if result.text.len() > 65_536 {
            return Err("reflex observation exceeded 65536 bytes".into());
        }
        Ok(result)
    });
    match result.and_then(|result| matches(reflex, &result).map(|trigger| (result, trigger))) {
        Ok((result, true)) => Evaluation::Wake(WakeReason::Reflex {
            id: reflex.id.clone(),
            observation: Observation {
                id: format!("reflex-{}-{now}", reflex.id),
                at: now,
                source: match &reflex.probe {
                    Some(Probe::Look { machine, inspect }) => crate::tools::source(machine, inspect),
                    Some(Probe::Web { url }) => url.clone(),
                    None => String::new(),
                },
                text: result.shown(),
                full_path: result.full_path,
            },
        }),
        Ok((_, false)) => Evaluation::Quiet,
        Err(reason) => Evaluation::Wake(WakeReason::Broken { id: reflex.id.clone(), reason: bounded(&reason, 4096) }),
    }
}
fn matches(reflex: &mut Reflex, result: &ReadResult) -> Result<bool, String> {
    match reflex.predicate.as_ref().ok_or("reflex has no predicate")? {
        Predicate::Contains { text } => Ok(result.text.lines().any(|line| line.contains(text))),
        Predicate::Absent { text } => Ok(!result.text.lines().any(|line| line.contains(text))),
        Predicate::Changed => {
            use std::hash::{Hash, Hasher};
            // This is a change detector, never an integrity or authorization proof.
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            result.text.hash(&mut hash);
            let next = format!("{:016x}", hash.finish());
            let changed = reflex.last_fingerprint.as_ref().is_some_and(|last| *last != next);
            reflex.last_fingerprint = Some(next);
            Ok(changed)
        }
        Predicate::Number { prefix, comparison, value } => {
            let number = number_after(&result.text, prefix)?;
            Ok(match comparison {
                Comparison::Less => number < *value,
                Comparison::LessEqual => number <= *value,
                Comparison::Greater => number > *value,
                Comparison::GreaterEqual => number >= *value,
                Comparison::Equal => number == *value,
            })
        }
    }
}

/// The number that follows `prefix` in the last line of `text` that holds
/// it, past spaces, colons and equals signs: `loss` reads 0.31 in
/// `step 1200 loss: 0.31`.
fn number_after(text: &str, prefix: &str) -> Result<f64, String> {
    let rest = text
        .lines()
        .rev()
        .find_map(|line| line.find(prefix).map(|at| &line[at + prefix.len()..]))
        .ok_or("no line holds the prefix")?;
    let rest = rest.trim_start_matches([' ', '\t', ':', '=']).as_bytes();
    let mut end = 0;
    for (index, &byte) in rest.iter().enumerate() {
        let exponent = matches!(byte, b'e' | b'E') && index > 0;
        let sign = matches!(byte, b'-' | b'+') && (index == 0 || matches!(rest[index - 1], b'e' | b'E'));
        if !(byte.is_ascii_digit() || byte == b'.' || exponent || sign) {
            break;
        }
        end = index + 1;
    }
    let number = std::str::from_utf8(&rest[..end])
        .ok()
        .and_then(|number| number.parse::<f64>().ok())
        .ok_or("no number follows the prefix")?;
    if !number.is_finite() {
        return Err("the number after the prefix is not finite".into());
    }
    Ok(number)
}
