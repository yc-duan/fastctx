//! Native submission order is independent of presentation and World splits.

use fastctx_types::Address;
use fastctx_types::session::{UserMessage, images};
use serde_json::Value;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Submissions {
    turn: String,
    ids: Vec<String>,
    #[serde(default)]
    complete: bool,
}

impl Submissions {
    pub fn observe(&mut self, turn: &str, item: &Value) -> Option<u32> {
        if item["type"] != "userMessage" {
            return None;
        }
        if self.turn != turn {
            self.turn = turn.into();
            self.ids.clear();
            self.complete = false;
        }
        let id = item["id"].as_str().expect("a native Codex user submission has an id");
        let index = match self.ids.iter().position(|previous| previous == id) {
            Some(index) => index,
            None => {
                self.ids.push(id.into());
                self.ids.len() - 1
            }
        };
        Some(u32::try_from(index).expect("a host submission ordinal fits u32"))
    }

    pub fn started(&mut self, turn: &str, subscribed: bool) {
        if self.turn != turn {
            self.turn = turn.into();
            self.ids.clear();
        }
        self.complete = subscribed;
    }

    pub fn known(&self, turn: &str, id: &str) -> Option<u32> {
        if !self.complete || self.turn != turn {
            return None;
        }
        self.ids
            .iter()
            .position(|known| known == id)
            .map(|index| u32::try_from(index).expect("a host submission ordinal fits u32"))
    }
}

pub(super) fn same_content(a: &UserMessage, b: &UserMessage) -> bool {
    a.client_id == b.client_id && text(a).eq(text(b)) && inline(a).eq(inline(b)) && local(a).eq(local(b))
}

fn text(message: &UserMessage) -> impl Iterator<Item = u8> + '_ {
    message
        .content
        .iter()
        .filter_map(|block| block["text"].as_str())
        .enumerate()
        .flat_map(|(index, text)| (index > 0).then_some(b'\n').into_iter().chain(text.as_bytes().iter().copied()))
}
fn inline(message: &UserMessage) -> impl Iterator<Item = (Option<&Value>, Option<&Value>)> {
    message.content.iter().filter(|block| block["type"] == "image").map(|block| {
        (block.get("url").filter(|value| !value.is_null()), block.get("fileId").filter(|value| !value.is_null()))
    })
}
fn local(message: &UserMessage) -> impl Iterator<Item = &Value> {
    message.content.iter().filter(|block| block["type"] == "localImage").map(|block| &block["path"])
}

pub(super) fn decorate(
    message: &mut UserMessage,
    owner: Option<&Address>,
    turn: &str,
    ordinal: Option<u32>,
    legacy: bool,
) {
    if let (Some(owner), Some(ordinal)) = (owner, ordinal) {
        images::codex(message, owner, turn, ordinal, legacy);
    }
}
