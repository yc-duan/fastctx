//! Older stored questions serialize as resident words so old IPC readers keep the full content.

use crate::{Address, goals::QuestionDelivery, session::Speaker};
use serde::{Serialize, Serializer};

pub(super) fn serialize<S: Serializer>(
    colleague: &Address,
    id: &str,
    text: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Words<'a> {
        from: Speaker,
        text: &'a str,
        goal_question: QuestionDelivery,
    }
    Words {
        from: Speaker::Resident { address: colleague.clone() },
        text,
        goal_question: QuestionDelivery { id: id.into(), colleague: colleague.clone(), account: None, questions: None },
    }
    .serialize(serializer)
}
