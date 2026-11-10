//! How a caller names work it submits.
//!
//! A call that asks for work taking time is answered once the work is
//! recorded, and the work is done after. Its caller gives the call a name,
//! the same every time it asks the same thing: asked again under the name,
//! for an answer that was lost or one not yet there, the work is not done a
//! second time, and the call is answered with how it stands. A caller that
//! names its call also says how long it stays for the outcome; past that it
//! is told the work is accepted (`Failure::accepted`) and asks again when
//! it wants to know.
//!
//! The name travels beside the call, not in it, so every call can carry one
//! and a machine that does not know names answers the call as it always did.

use serde::{Deserialize, Serialize};

/// The longest name of a submission.
pub const NAME_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Submission {
    /// What the caller calls this one request: unguessable, and reused only
    /// to ask the same thing again.
    pub name: String,
    /// How long the caller stays for the outcome, in milliseconds; the
    /// machine answering may stay less.
    #[serde(default)]
    pub waits_ms: u32,
}

impl Submission {
    /// A name of 1 to `NAME_BYTES` ASCII letters, digits, `-` and `_`.
    pub fn check(&self) -> Result<(), String> {
        let named = !self.name.is_empty()
            && self.name.len() <= NAME_BYTES
            && self.name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if named {
            Ok(())
        } else {
            Err(format!("a submission's name is 1 to {NAME_BYTES} ASCII letters, digits, '-' or '_'"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_bounded_plain_words() {
        let named = |name: &str| Submission { name: name.into(), waits_ms: 0 }.check().is_ok();
        assert!(named("a"));
        assert!(named(&"f".repeat(NAME_BYTES)));
        assert!(named("0190a7c2-77aa_Z"));
        assert!(!named(""));
        assert!(!named(&"f".repeat(NAME_BYTES + 1)));
        assert!(!named("a/b"));
        assert!(!named("名字"));
    }

    #[test]
    fn a_name_without_a_wait_does_not_stay() {
        let read: Submission = serde_json::from_str(r#"{"name":"n"}"#).unwrap();
        assert_eq!(read, Submission { name: "n".into(), waits_ms: 0 });
    }
}
