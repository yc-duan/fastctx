//! What the hub writes to each member on its control connection, counted by
//! kind as each frame is encoded (`control::encoding`): where the hub's line
//! goes. The hub's health names the member and kind pairs that took the
//! most since the hub started and since its health was last written, and
//! each kind's total, so it stays small however large the network.

use std::collections::HashMap;
use std::sync::Mutex;

use fastctx_net::wire::{Down, Told};
use fastctx_types::MachineName;
use fastctx_types::assets::{Push, Reply};
use serde_json::json;

/// How many member and kind pairs each list of the health names.
const NAMED: usize = 12;

/// The frames and bytes the hub wrote, by member and kind.
#[derive(Default)]
pub(crate) struct Egress {
    counts: Mutex<Counts>,
}

#[derive(Default)]
struct Counts {
    sent: HashMap<(MachineName, &'static str), Tally>,
    /// `sent` as the health last read it.
    read: HashMap<(MachineName, &'static str), Tally>,
}

#[derive(Clone, Copy, Default)]
struct Tally {
    frames: u64,
    bytes: u64,
}

impl Egress {
    /// `member` was written these frames: each one's kind and bytes.
    pub(crate) fn wrote(&self, member: &MachineName, written: &[(&'static str, usize)]) {
        if written.is_empty() {
            return;
        }
        let mut counts = self.counts.lock().expect("never poisoned");
        for (kind, bytes) in written {
            let tally = counts.sent.entry((member.clone(), *kind)).or_default();
            tally.frames += 1;
            tally.bytes += *bytes as u64;
        }
    }

    /// The member and kind pairs that took the most, since the hub started
    /// and since the last look, and each kind's total.
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let mut counts = self.counts.lock().expect("never poisoned");
        let mut kinds: HashMap<&'static str, Tally> = HashMap::new();
        let mut most = Vec::with_capacity(counts.sent.len());
        let mut lately = Vec::new();
        for ((member, kind), tally) in &counts.sent {
            let total = kinds.entry(kind).or_default();
            total.frames += tally.frames;
            total.bytes += tally.bytes;
            most.push((tally.bytes, tally.frames, member.as_str(), *kind));
            let before = counts.read.get(&(member.clone(), *kind)).copied().unwrap_or_default();
            if tally.bytes > before.bytes {
                lately.push((tally.bytes - before.bytes, tally.frames - before.frames, member.as_str(), *kind));
            }
        }
        let named = |mut rows: Vec<(u64, u64, &str, &'static str)>| {
            rows.sort_unstable_by(|a, b| b.0.cmp(&a.0));
            rows.truncate(NAMED);
            rows.into_iter()
                .map(|(bytes, frames, member, kind)| json!({ "member": member, "kind": kind, "bytes": bytes, "frames": frames }))
                .collect::<Vec<_>>()
        };
        let (most, lately) = (named(most), named(lately));
        let mut kinds: Vec<_> = kinds.into_iter().collect();
        kinds.sort_unstable_by(|a, b| b.1.bytes.cmp(&a.1.bytes));
        let kinds: Vec<_> = kinds
            .into_iter()
            .map(|(kind, tally)| json!({ "kind": kind, "bytes": tally.bytes, "frames": tally.frames }))
            .collect();
        counts.read = counts.sent.clone();
        json!({ "most": most, "lately": lately, "kinds": kinds })
    }
}

/// What `frame` is, as the hub's health names it. A part of an answer too
/// large for one frame is named by the answer it is of, as its queue knew
/// it (`crate::outq::Outq::part_kind`).
pub(crate) fn kind(frame: &Down) -> &'static str {
    match frame {
        Down::Welcome { .. } => "welcome",
        Down::LoginResponse { .. } => "login",
        Down::TokenInfo { .. } => "token",
        Down::RegisterResponse { .. } => "register",
        Down::LoggedIn { .. } => "logged_in",
        Down::Registered => "registered",
        Down::Paired => "paired",
        Down::Refused { .. } => "refused",
        Down::Pong { .. } => "pong",
        Down::Ack { .. } => "ack",
        Down::Deliver { .. } => "letter",
        Down::Incoming { .. } => "relayed_call",
        Down::Status { .. } => "call_status",
        Down::Answered { .. } => "relayed_answer",
        Down::Told { answer, .. } => told(answer),
        Down::DirectoryChanged => "directory_changed",
        Down::Ready => "ready",
        Down::ReplicasChanged => "replicas_changed",
        Down::HistoryChanged { .. } => "history_changed",
        Down::LedgerChanged { .. } => "ledger_changed",
        Down::Assets { push } => match push {
            Push::Moved { .. } => "assets/moved",
            Push::Checkpointed { .. } => "assets/checkpointed",
            Push::Fact { .. } => "assets/fact",
            Push::Revoked { .. } => "assets/revoked",
        },
        Down::AccountRecordChanged { .. } => "account_record_changed",
        Down::Afoot { .. } => "afoot",
        Down::Forget { .. } => "forget",
        Down::ForgetAccount { .. } => "forget_account",
        Down::Purge { .. } => "purge",
        Down::Versions { .. } | Down::VersionPart { .. } => "versions",
        Down::Fetch { .. } | Down::FetchCancelled { .. } => "fetch",
        Down::PackageProgress { .. } => "package_progress",
        Down::Cancelled { .. } => "cancelled",
        Down::Presence { .. } => "presence",
        Down::Part { .. } => "part",
        Down::View { .. } => "view",
    }
}

/// What an answer is, as the hub's health names it.
pub(crate) fn told(answer: &Result<Told, String>) -> &'static str {
    let Ok(told) = answer else { return "told/error" };
    match told {
        Told::Assets { reply } => assets(reply),
        Told::SkillFiles { .. } => "told/skill_files",
        Told::PlanCopies { .. } => "told/plan_copies",
        Told::OwnedRecords { .. } => "told/owned_records",
        Told::AccountRecords { .. } => "told/account_records",
        Told::HistorySubmitted { .. } => "told/history_submitted",
        Told::HistoryCatalog { .. } => "told/history_catalog",
        Told::HistoryDocument { .. } => "told/history_document",
        Told::ProjectIndex { .. } => "told/project_index",
        Told::MemoryDocument { .. } => "told/memory_document",
        Told::NetworkUpdates { .. } => "told/network_updates",
        Told::MachineLog { .. } => "told/machine_log",
        Told::CollabWritten { .. } => "told/collab_written",
        Told::ProfileWritten { .. } => "told/profile_written",
        Told::KernelRelease { .. } => "told/kernel_release",
        Told::Directory { .. } => "told/directory",
        Told::Done => "told/done",
        Told::Accepted => "told/accepted",
        Told::Package { .. } => "told/package",
        Told::Token { .. } => "told/token",
        Told::RegisterResponse { .. } => "told/register",
        Told::Written { .. } => "told/written",
        Told::Towns { .. } => "told/towns",
        Told::LedgerProjection { .. } => "told/ledger_projection",
        Told::Entries { .. } => "told/entries",
        Told::HubMove { .. } => "told/hub_move",
        Told::HubIdentity { .. } => "told/hub_identity",
    }
}

/// What an answer about assets is.
fn assets(reply: &Reply) -> &'static str {
    match reply {
        Reply::Ready { .. } => "told/assets/ready",
        Reply::Sealed { .. } => "told/assets/sealed",
        Reply::HandedBack { .. } => "told/assets/handed_back",
        Reply::Revoked { .. } => "told/assets/revoked",
        Reply::Cleared { .. } => "told/assets/cleared",
        Reply::Refused { .. } => "told/assets/refused",
        Reply::Received { .. } | Reply::ReceivedMany { .. } => "told/assets/received",
        Reply::Word { .. } => "told/assets/word",
        Reply::Standing { .. } => "told/assets/standing",
        Reply::Frames { .. } => "told/assets/frames",
        Reply::Staged { .. } => "told/assets/staged",
        Reply::Holdings { .. } => "told/assets/holdings",
        Reply::Succession { .. } => "told/assets/succession",
        Reply::Described { .. } => "told/assets/described",
        Reply::Streams { .. } => "told/assets/streams",
        Reply::Changes { .. } => "told/assets/changes",
        Reply::Window { .. } => "told/assets/window",
        Reply::Older { .. } => "told/assets/older",
        Reply::Reported => "told/assets/reported",
        Reply::SyncStatus { .. } => "told/assets/sync_status",
        Reply::Epoch { .. } => "told/assets/epoch",
        Reply::HandedOn { .. } => "told/assets/handed_on",
        Reply::Put { .. } => "told/assets/put",
        Reply::Get { .. } => "told/assets/get",
        Reply::Challenge { .. } => "told/assets/challenge",
        Reply::Proof { .. } => "told/assets/proof",
        Reply::Noted => "told/assets/noted",
        Reply::Permits { .. } => "told/assets/permits",
        Reply::Vouched { .. } => "told/assets/vouched",
    }
}
