//! Session bookkeeping that lets one MCP session outlive the engine link carrying it.
//!
//! The proxy forwards bytes; it does not interpret them. This module adds the smallest amount of
//! interpretation that a resumable session needs: which requests are still unanswered, and what a
//! replacement engine must be told before it can take over. Everything here is a side channel —
//! the bytes on the wire are unchanged, so a session that never loses its link pays only the scan.

use serde_json::Value;

/// Bytes of a single message inspected for its JSON-RPC envelope fields.
///
/// `id` is the second field rmcp serialises, and a request carries it before its parameters, so
/// this window holds the envelope of every realistic message. A message whose id falls outside it
/// stays untracked, which costs at most one duplicate answer after a relink.
const ENVELOPE_WINDOW_BYTES: usize = 64 * 1024;
/// Upper bound on the incomplete request line retained for replay onto a replacement link.
const MAX_PARTIAL_REQUEST_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound on the cached session prologue. A Codex `initialize` is roughly one kilobyte.
const MAX_PROLOGUE_BYTES: usize = 1024 * 1024;
/// Upper bound on tracked unanswered requests. Hosts keep a handful of tool calls in flight.
const MAX_UNANSWERED_REQUESTS: usize = 4096;
/// Capacity the line buffers return to once a message completes, so one large request or answer
/// does not permanently raise the proxy's resident size.
const LINE_BUFFER_IDLE_CAPACITY: usize = 8 * 1024;

const INITIALIZE_METHOD: &str = "initialize";
const INITIALIZED_NOTIFICATION: &str = "notifications/initialized";

/// A request the engine received or was about to receive, still waiting for its answer.
pub(super) struct UnansweredRequest {
    id: Value,
    /// Whether the bytes completing this request reached the engine before the link failed.
    delivered: bool,
}

impl UnansweredRequest {
    /// Renders the JSON-RPC error line that closes out this request on the host's stdout.
    ///
    /// A delivered request may have run to completion with its side effects intact, so the text
    /// says so rather than inviting a blind retry; an undelivered one provably did not run.
    pub(super) fn failure_line(&self) -> Vec<u8> {
        let message = if self.delivered {
            "The FastCtx control center stopped while this call was running, so it has no result. \
             The call may or may not have finished; check the current state before running it \
             again."
        } else {
            "The FastCtx control center stopped before this call reached it. Nothing ran; run it \
             again."
        };
        let mut line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.id,
            "error": { "code": -32603, "message": message },
        })
        .to_string()
        .into_bytes();
        line.push(b'\n');
        line
    }
}

/// What one side of the session has seen so far on a newline-delimited JSON-RPC stream.
struct LineBuffer {
    bytes: Vec<u8>,
    /// Set once a message outgrows the envelope window: the rest is forwarded but not inspected.
    truncated: bool,
}

impl LineBuffer {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(LINE_BUFFER_IDLE_CAPACITY),
            truncated: false,
        }
    }

    fn reset(&mut self) {
        self.bytes.clear();
        self.bytes.shrink_to(LINE_BUFFER_IDLE_CAPACITY);
        self.truncated = false;
    }
}

/// Per-session state shared across every engine link that carries the session.
pub(super) struct SessionJournal {
    /// Raw `initialize` request line, replayed so a replacement engine accepts the session.
    initialize: Option<Vec<u8>>,
    /// Canonical text of the `initialize` request id, used to route its replayed answer.
    initialize_id: Option<String>,
    /// Whether the host is still waiting for its `initialize` answer.
    initialize_owed: bool,
    /// Raw `notifications/initialized` line, replayed after `initialize`.
    initialized: Option<Vec<u8>>,
    /// Set when a prologue message could not be cached, which makes the session unresumable.
    prologue_lost: Option<String>,
    unanswered: Vec<(String, UnansweredRequest)>,
    /// Set when the unanswered set overflowed and can no longer close out every request.
    tracking_lost: bool,
    request_line: LineBuffer,
    answer_line: LineBuffer,
    /// Bytes of the request line in progress, replayed so a replacement engine sees it whole.
    partial_request: Vec<u8>,
    /// Set when the in-progress request outgrew replay and its bytes must be resynchronised.
    partial_request_lost: bool,
    forwarded: bool,
}

impl SessionJournal {
    pub(super) fn new() -> Self {
        Self {
            initialize: None,
            initialize_id: None,
            initialize_owed: false,
            initialized: None,
            prologue_lost: None,
            unanswered: Vec::new(),
            tracking_lost: false,
            request_line: LineBuffer::new(),
            answer_line: LineBuffer::new(),
            partial_request: Vec::new(),
            partial_request_lost: false,
            forwarded: false,
        }
    }

    /// Whether the host has asked anything yet, which is how a closing session tells "answers are
    /// owed" from "the client opened the transport and changed its mind".
    pub(super) fn has_forwarded(&self) -> bool {
        self.forwarded
    }

    /// Records request bytes on their way to the engine. Call before writing them.
    pub(super) fn observe_request_bytes(&mut self, chunk: &[u8]) {
        self.forwarded = true;
        for &byte in chunk {
            self.push_request_byte(byte);
        }
    }

    fn push_request_byte(&mut self, byte: u8) {
        if self.partial_request.len() < MAX_PARTIAL_REQUEST_BYTES {
            self.partial_request.push(byte);
        } else {
            self.partial_request_lost = true;
        }
        if byte != b'\n' {
            if self.request_line.bytes.len() < ENVELOPE_WINDOW_BYTES {
                self.request_line.bytes.push(byte);
            } else {
                self.request_line.truncated = true;
            }
            return;
        }
        self.complete_request_line();
    }

    fn complete_request_line(&mut self) {
        let head = envelope_head(&self.request_line.bytes);
        match (&head.method, &head.id) {
            (Some(method), Some(id)) => {
                let key = canonical_id(id);
                if method == INITIALIZE_METHOD {
                    self.cache_initialize(key.clone());
                }
                self.track_request(key, id.clone());
            }
            (Some(method), None) if method == INITIALIZED_NOTIFICATION => {
                self.cache_initialized();
            }
            _ => {}
        }
        self.request_line.reset();
        self.partial_request.clear();
        self.partial_request.shrink_to(LINE_BUFFER_IDLE_CAPACITY);
        self.partial_request_lost = false;
    }

    fn cache_initialize(&mut self, key: String) {
        if self.request_line.truncated || self.partial_request_lost {
            self.prologue_lost = Some(
                "the host's initialize request was too large to retain for a reconnect".to_string(),
            );
            return;
        }
        if self.partial_request.len() > MAX_PROLOGUE_BYTES {
            self.prologue_lost = Some(
                "the host's initialize request exceeded the reconnect replay limit".to_string(),
            );
            return;
        }
        self.initialize = Some(self.partial_request.clone());
        self.initialize_id = Some(key);
        self.initialize_owed = true;
    }

    fn cache_initialized(&mut self) {
        if self.request_line.truncated || self.partial_request_lost {
            return;
        }
        if self.partial_request.len() <= MAX_PROLOGUE_BYTES {
            self.initialized = Some(self.partial_request.clone());
        }
    }

    fn track_request(&mut self, key: String, id: Value) {
        if self.unanswered.iter().any(|(tracked, _)| tracked == &key) {
            return;
        }
        if self.unanswered.len() >= MAX_UNANSWERED_REQUESTS {
            self.tracking_lost = true;
            return;
        }
        self.unanswered.push((
            key,
            UnansweredRequest {
                id,
                delivered: false,
            },
        ));
    }

    /// Marks every request observed so far as delivered. Call after the write that carried them
    /// to the engine succeeded.
    pub(super) fn mark_delivered(&mut self) {
        for (_, request) in &mut self.unanswered {
            request.delivered = true;
        }
    }

    /// Records answer bytes on their way to the host.
    pub(super) fn observe_answer_bytes(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if byte != b'\n' {
                if self.answer_line.bytes.len() < ENVELOPE_WINDOW_BYTES {
                    self.answer_line.bytes.push(byte);
                } else {
                    self.answer_line.truncated = true;
                }
                continue;
            }
            let head = envelope_head(&self.answer_line.bytes);
            if head.method.is_none()
                && let Some(id) = head.id
            {
                self.resolve(&canonical_id(&id));
            }
            self.answer_line.reset();
        }
    }

    fn resolve(&mut self, key: &str) {
        if self.initialize_id.as_deref() == Some(key) {
            self.initialize_owed = false;
        }
        self.unanswered.retain(|(tracked, _)| tracked != key);
    }

    /// Removes and returns every request the failed link can no longer answer.
    ///
    /// The `initialize` request stays behind: a replacement link replays it and delivers the real
    /// answer, which is what the host is actually waiting for.
    pub(super) fn take_unanswered(&mut self) -> Vec<UnansweredRequest> {
        let initialize = self.initialize_id.clone();
        let keep_initialize = self.initialize_owed;
        let mut taken = Vec::new();
        let mut retained = Vec::new();
        for (key, request) in std::mem::take(&mut self.unanswered) {
            if keep_initialize && initialize.as_deref() == Some(key.as_str()) {
                retained.push((key, request));
            } else {
                taken.push(request);
            }
        }
        self.unanswered = retained;
        taken
    }

    /// Whether closing out unanswered requests can be complete, or silently missed some.
    pub(super) fn tracking_is_complete(&self) -> bool {
        !self.tracking_lost
    }

    /// Why this session can never be moved to another engine, if that is the case.
    ///
    /// Permanent by nature: no retry can recover a handshake that was never retained. Callers use
    /// it to stop before entering a retry loop that can only fail the same way every round.
    pub(super) fn resume_blocked(&self) -> Option<String> {
        self.prologue_lost.as_ref().map(|reason| {
            format!("The FastCtx session cannot be moved to a new control center because {reason}.")
        })
    }

    /// The bytes a replacement engine needs before it can carry this session.
    pub(super) fn resume_plan(&self) -> Result<ResumePlan, String> {
        if let Some(reason) = self.resume_blocked() {
            return Err(reason);
        }
        Ok(ResumePlan {
            initialize: self.initialize.clone(),
            initialized: self.initialized.clone(),
            initialize_owed: self.initialize_owed,
            partial_request: if self.partial_request_lost {
                None
            } else {
                Some(self.partial_request.clone())
            },
        })
    }

    /// Accepts the answer a replacement engine gave to the replayed `initialize`.
    pub(super) fn accept_replayed_initialize(&mut self) {
        self.initialize_owed = false;
        if let Some(key) = self.initialize_id.clone() {
            self.unanswered.retain(|(tracked, _)| tracked != &key);
        }
    }

    /// Drops the request bytes that a replacement engine will never receive whole, so the next
    /// newline resynchronises the stream instead of handing it a spliced message.
    pub(super) fn abandon_partial_request(&mut self) {
        self.partial_request.clear();
        self.partial_request.shrink_to(LINE_BUFFER_IDLE_CAPACITY);
        self.partial_request_lost = false;
    }
}

/// What a replacement engine must be told before the session resumes on it.
pub(super) struct ResumePlan {
    /// Raw `initialize` request line, absent when the link failed before the host sent one.
    pub(super) initialize: Option<Vec<u8>>,
    pub(super) initialized: Option<Vec<u8>>,
    /// Whether the answer to the replayed `initialize` is still owed to the host.
    pub(super) initialize_owed: bool,
    /// Bytes of the request line in progress. `None` when it outgrew replay and the stream has to
    /// resynchronise on the next newline instead.
    pub(super) partial_request: Option<Vec<u8>>,
}

/// The JSON-RPC envelope fields the session needs, read without materialising the payload.
struct EnvelopeHead {
    id: Option<Value>,
    method: Option<String>,
}

/// Reads the top-level `id` and `method` of one JSON-RPC message.
///
/// A raw newline never appears inside a JSON string, so callers can split messages on newlines
/// before reaching here. Anything that is not a JSON object — a batch array, a partial message
/// past the envelope window — yields no fields and stays untracked.
fn envelope_head(message: &[u8]) -> EnvelopeHead {
    let mut head = EnvelopeHead {
        id: None,
        method: None,
    };
    let mut cursor = skip_whitespace(message, 0);
    if message.get(cursor) != Some(&b'{') {
        return head;
    }
    cursor += 1;
    loop {
        cursor = skip_whitespace(message, cursor);
        match message.get(cursor) {
            Some(b'}') | None => return head,
            Some(b',') => {
                cursor += 1;
                continue;
            }
            Some(b'"') => {}
            Some(_) => return head,
        }
        let Some((key, after_key)) = read_string(message, cursor) else {
            return head;
        };
        cursor = skip_whitespace(message, after_key);
        if message.get(cursor) != Some(&b':') {
            return head;
        }
        cursor = skip_whitespace(message, cursor + 1);
        let Some(after_value) = skip_value(message, cursor) else {
            return head;
        };
        match key.as_str() {
            "id" => head.id = serde_json::from_slice(&message[cursor..after_value]).ok(),
            "method" => head.method = serde_json::from_slice(&message[cursor..after_value]).ok(),
            _ => {}
        }
        if head.id.is_some() && head.method.is_some() {
            return head;
        }
        cursor = after_value;
    }
}

fn skip_whitespace(message: &[u8], mut cursor: usize) -> usize {
    while matches!(message.get(cursor), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        cursor += 1;
    }
    cursor
}

/// Reads a JSON string starting at an opening quote, returning its value and the index after it.
fn read_string(message: &[u8], cursor: usize) -> Option<(String, usize)> {
    let end = skip_string(message, cursor)?;
    let value: String = serde_json::from_slice(&message[cursor..end]).ok()?;
    Some((value, end))
}

/// Returns the index just past a JSON string starting at an opening quote.
fn skip_string(message: &[u8], cursor: usize) -> Option<usize> {
    if message.get(cursor) != Some(&b'"') {
        return None;
    }
    let mut index = cursor + 1;
    while let Some(&byte) = message.get(index) {
        match byte {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Returns the index just past one JSON value, skipping nested structures without decoding them.
fn skip_value(message: &[u8], cursor: usize) -> Option<usize> {
    match *message.get(cursor)? {
        b'"' => skip_string(message, cursor),
        b'{' | b'[' => skip_container(message, cursor),
        // A scalar runs until the next structural byte.
        _ => {
            let mut index = cursor;
            while let Some(&byte) = message.get(index) {
                if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                    break;
                }
                index += 1;
            }
            (index > cursor).then_some(index)
        }
    }
}

/// Returns the index just past a JSON object or array starting at its opening bracket.
fn skip_container(message: &[u8], cursor: usize) -> Option<usize> {
    let mut index = cursor;
    let mut depth = 0_usize;
    loop {
        match *message.get(index)? {
            b'"' => {
                index = skip_string(message, index)?;
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
}

/// Normalises a JSON-RPC id so the request side and the answer side agree on one key.
fn canonical_id(id: &Value) -> String {
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::{SessionJournal, envelope_head};

    fn line(text: &str) -> Vec<u8> {
        format!("{text}\n").into_bytes()
    }

    #[test]
    fn envelope_fields_are_read_without_decoding_the_payload() {
        let head = envelope_head(
            br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"id":"nested"}}"#,
        );
        assert_eq!(head.id.unwrap(), serde_json::json!(7));
        assert_eq!(head.method.unwrap(), "tools/call");

        let answer = envelope_head(br#"{"jsonrpc":"2.0","id":"abc","result":{"content":[]}}"#);
        assert_eq!(answer.id.unwrap(), serde_json::json!("abc"));
        assert!(answer.method.is_none());

        assert!(envelope_head(br#"[{"jsonrpc":"2.0"}]"#).id.is_none());
    }

    #[test]
    fn an_answered_request_is_no_longer_owed_but_an_unanswered_one_is_closed_out() {
        let mut journal = SessionJournal::new();
        journal.observe_request_bytes(&line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#));
        journal.observe_request_bytes(&line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#));
        journal.observe_request_bytes(&line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call"}"#));
        journal.mark_delivered();
        journal.observe_answer_bytes(&line(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#));

        let unanswered = journal.take_unanswered();

        assert_eq!(unanswered.len(), 1);
        let failure = String::from_utf8(unanswered[0].failure_line()).unwrap();
        assert!(failure.contains("\"id\":3"), "{failure}");
        assert!(
            failure.contains("may or may not have finished"),
            "{failure}"
        );
        // initialize stays behind for the replacement engine to answer for real.
        let plan = journal.resume_plan().unwrap();
        assert!(plan.initialize_owed);
        assert!(plan.initialize.is_some());
    }

    #[test]
    fn an_undelivered_request_is_reported_as_never_having_run() {
        let mut journal = SessionJournal::new();
        journal.observe_request_bytes(&line(r#"{"jsonrpc":"2.0","id":9,"method":"tools/call"}"#));

        let unanswered = journal.take_unanswered();

        let failure = String::from_utf8(unanswered[0].failure_line()).unwrap();
        assert!(failure.contains("Nothing ran"), "{failure}");
    }

    #[test]
    fn an_incomplete_request_is_replayed_so_the_next_engine_sees_it_whole() {
        let mut journal = SessionJournal::new();
        journal.observe_request_bytes(&line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#));
        journal.observe_request_bytes(br#"{"jsonrpc":"2.0","id":2,"met"#);

        let plan = journal.resume_plan().unwrap();

        assert_eq!(
            plan.partial_request.unwrap(),
            br#"{"jsonrpc":"2.0","id":2,"met"#.to_vec()
        );
    }

    #[test]
    fn a_session_without_an_initialize_answer_keeps_owing_it() {
        let mut journal = SessionJournal::new();
        journal.observe_request_bytes(&line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#));
        journal.observe_answer_bytes(&line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#));

        let plan = journal.resume_plan().unwrap();

        assert!(!plan.initialize_owed);
        assert!(journal.take_unanswered().is_empty());
    }
}
