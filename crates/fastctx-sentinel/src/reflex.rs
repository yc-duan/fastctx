//! Reflexes evaluate bounded observations without invoking a model.
use crate::ports::{Look, ReadResult, Web};
use fastctx_agent::runtime::bounded;
use fastctx_types::sentinel::*;

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
    if reflex.every_ms.is_some_and(|v| !(1000..=31_536_000_000).contains(&v)) {
        return Err("invalid reflex interval".into());
    }
    match (&reflex.probe, &reflex.predicate) {
        (None, None) => Ok::<(), String>(()),
        (Some(Probe::Look { command, .. }), Some(_))
            if kind == Kind::Local && !command.is_empty() && command.len() <= 8192 =>
        {
            Ok(())
        }
        (Some(Probe::Web { url }), Some(test))
            if kind == Kind::Web && url.len() <= 4096 && !matches!(test, Predicate::ExitNonzero) =>
        {
            Ok(())
        }
        _ => Err("reflex probe is not available to this sentinel".into()),
    }?;
    match &reflex.predicate {
        Some(Predicate::Contains { text }) if text.is_empty() || text.len() > 4096 => Err("invalid reflex text".into()),
        Some(Predicate::Number { prefix, value, .. }) if prefix.len() > 4096 || !value.is_finite() => {
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
    match tokio::time::timeout(std::time::Duration::from_secs(30), evaluate_inner(definition, reflex, now, look, web))
        .await
    {
        Ok(result) => result,
        Err(_) => Evaluation::Wake(WakeReason::Broken {
            id: reflex.id.clone(),
            reason: "reflex timed out after 30 seconds".into(),
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
        Some(Probe::Look { machine, command }) if definition.kind == Kind::Local => {
            look.look(definition, machine, command).await
        }
        Some(Probe::Web { url }) if definition.kind == Kind::Web => web.fetch(url).await,
        _ => Err("reflex capability does not match sentinel kind".into()),
    };
    let result = result.and_then(|result| {
        if result.text.len() > 65_536 {
            return Err("reflex observation exceeded 65536 bytes".into());
        }
        if result.exit_code.is_some_and(|code| code != 0) && !matches!(reflex.predicate, Some(Predicate::ExitNonzero)) {
            return Err(format!("command failed: {:?}: {}", result.exit_code, bounded(&result.text, 2048)));
        }
        Ok(result)
    });
    match result.and_then(|result| matches(reflex, &result).map(|trigger| (result, trigger))) {
        Ok((result, true)) => Evaluation::Wake(WakeReason::Reflex {
            id: reflex.id.clone(),
            observation: Observation {
                id: format!("reflex-{}-{now}", reflex.id),
                at: now,
                source: serde_json::to_string(&reflex.probe).expect("reflex probe serializes"),
                text: result.text,
                exit_code: result.exit_code,
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
        Predicate::ExitNonzero => Ok(result.exit_code.is_some_and(|code| code != 0)),
        Predicate::Changed => {
            use std::hash::{Hash, Hasher};
            // This is a change detector, never an integrity or authorization proof.
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            result.text.hash(&mut hash);
            result.exit_code.hash(&mut hash);
            let next = format!("{:016x}", hash.finish());
            let changed = reflex.last_fingerprint.as_ref().is_some_and(|last| *last != next);
            reflex.last_fingerprint = Some(next);
            Ok(changed)
        }
        Predicate::Number { prefix, comparison, value } => {
            let line = result
                .text
                .lines()
                .find_map(|line| line.strip_prefix(prefix))
                .ok_or("numeric source no longer contains the prefix")?;
            let number = line
                .split_whitespace()
                .next()
                .ok_or("numeric source is empty")?
                .parse::<f64>()
                .map_err(|_| "numeric source is no longer a number")?;
            if !number.is_finite() {
                return Err("numeric source is not finite".into());
            }
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
