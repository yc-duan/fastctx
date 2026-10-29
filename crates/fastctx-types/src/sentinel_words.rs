//! Sentinel command and delivery wording, collected for editorial review.
pub const HELP: &str = r#"fastctx sense '<what to wait for>' [--at <time>]... [--web] [--tell '<note>']

Leave a sentinel to wait for something and wake you when it happens. It
lives on <machine>, which stays on, so it keeps watching after this session
ends or this computer sleeps, and a closed session is brought back to hear it.

Say the thing in one sentence, as exactly as you can: what must become true,
and where to look if you know. The sentinel works out how to watch it and
replies with its plan and a first look; read the plan.

A sentinel sees the user's machines and the connections allowed for it,
read-only, and cannot reach the internet.

With --web it is the other way round: it searches and reads the public web
and sees nothing of the user's: no machines, no files, no connections. What
you write goes into web searches, so write nothing private. Use --web only
for what is on the public web alone, such as a published price.

--at also wakes you at that time and keeps waiting; repeat it for more
times: 08:00, 2026-10-23T08:00, 30m. When the thing happens you are woken
once and the sentinel ends; without --at it waits until then. With --at and
no sentence, it is a plain alarm.

--tell is the line you will read when it wakes you.

Your sentinels are in `fastctx w`; `fastctx stop <id>` removes one.
"#;
pub const NO_HOME: &str =
    "sentinels are not turned on for this account; the user turns them on in the Fastctx app, under Features > Sentinels";
pub const NO_MODEL: &str =
    "sentinels have no model yet; the user chooses one in the Fastctx app, under Features > Sentinels";
pub const NO_KEY: &str =
    "the sentinels' model has no key yet; the user enters it in the Fastctx app, under Features > Sentinels";
pub const NEED_CONVERSATION: &str = "a conversation registers a sentinel for itself";
pub const ON_ITS_MACHINE: &str =
    "a sentinel is registered on the machine the conversation lives on; run fastctx sense there, not inside on";
pub const CHOOSE_ACCOUNT: &str = "choose the sentinel account with --as and its hub with --hub";
pub fn home_unavailable(status: &crate::sentinel_settings::HomeStatus) -> String {
    use crate::sentinel_settings::HomeStatus;
    match status {
        HomeStatus::Unset | HomeStatus::Ready => NO_HOME.to_string(),
        HomeStatus::Forbidden => "this account may no longer use the machine its sentinels live on; the user chooses another in the Fastctx app, under Features > Sentinels".into(),
        HomeStatus::Offline => "the machine the sentinels live on is offline; a sentinel can be left once it is back".into(),
        HomeStatus::TooOld => "the machine the sentinels live on runs an earlier release; update it to leave a sentinel".into(),
        HomeStatus::Unavailable { reason } => format!("the machine the sentinels live on cannot be asked now: {reason}"),
    }
}
pub fn created(summary: &crate::sentinel_control::Summary) -> String {
    let mut text =
        format!("{} on {}: {:?}\n{}\n", summary.definition.id, summary.definition.home, summary.phase, summary.plan);
    if let Some(observation) = &summary.latest {
        text.push_str(&format!("{}\n", observation.text));
    }
    if let Some(reason) = &summary.reason {
        text.push_str(&format!("{reason}\n"));
    }
    text
}
pub fn stopped(id: &crate::SentinelId, reason: Option<&str>) -> String {
    match reason {
        None => format!("{id}: withdrawn\n"),
        Some(reason) => format!("{id}: withdrawn; home acknowledgement pending: {reason}\n"),
    }
}

pub fn row(summary: &crate::sentinel_control::Summary) -> String {
    let line = summary.definition.condition.lines().next().unwrap_or_default();
    let condition = if line.chars().count() > 100 {
        format!("{} …", line.chars().take(100).collect::<String>())
    } else {
        line.to_string()
    };
    format!(
        "  {}.{}  {}  {:?}  {}  {}\n",
        summary.definition.home,
        summary.definition.hub,
        summary.definition.id,
        summary.phase,
        summary.definition.owner.resident,
        condition
    )
}

/// The event remains stable on disk; lateness is calculated when it is read.
pub fn notification(observation: &crate::sentinel_notice::SentinelNotice, now: jiff::Timestamp) -> String {
    use crate::sentinel_notice::Event;
    let (what, reason, evidence) = match &observation.event {
        Event::Alarm { .. } => ("alarm time reached", "", &[][..]),
        Event::Satisfied {reason,evidence} => ("condition satisfied", reason.as_str(), evidence.as_slice()),
        Event::Unable {reason} => ("unable to watch",reason.as_str(), &[][..]),
    };
    let late=now.as_millisecond().saturating_sub(observation.due.as_millisecond()).max(0)/1000;
    let mut text=format!("=== sentinel {} on {}: {what} at {}; {late}s late ===\n",observation.sentinel,observation.machine,observation.at);
    if !observation.condition.is_empty() {text.push_str(&format!("Watching: {}\n",observation.condition));}
    if !observation.tell.is_empty() {text.push_str(&format!("Your note: {}\n",observation.tell));}
    if !reason.is_empty() {text.push_str(reason);text.push('\n');}
    for seen in evidence {
        let at = jiff::Timestamp::from_millisecond(seen.at).map_or_else(|_| seen.at.to_string(), |at| at.to_string());
        text.push_str(&format!("{} at {at}: {}\n",seen.source,seen.text.chars().take(2048).collect::<String>()));
        if let Some(path)=&seen.full_path {text.push_str(&format!("Full observation on {}: {path}\n",observation.machine));}
    }
    text
}
