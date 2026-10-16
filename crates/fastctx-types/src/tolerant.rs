//! Reading what a later release may say (`world-versions.md`, 读得懂比自己新
//! 的): a value of a closed list, met where one machine tells another or an
//! interface how things stand, may be one a newer release added. Read there,
//! a word this release does not know becomes the most careful value it
//! knows, rather than failing the whole message it came in. A change asked
//! is read strictly: what is not understood is refused, never done as
//! something else.

use serde::{Deserialize, Deserializer, Serialize};

/// A closed list a later release may add to, and what a word of it this
/// release does not know is read as.
pub trait Tolerant: Copy + Serialize + 'static {
    /// Every value this release knows.
    const ALL: &'static [Self];
    /// What an unknown word is read as: the value that gives it least.
    const UNKNOWN: Self;

    /// The value named `word`, as this release writes them.
    fn named(word: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|value| serde_json::to_value(value).ok().as_ref().and_then(serde_json::Value::as_str) == Some(word))
            .unwrap_or(Self::UNKNOWN)
    }
}

/// A field of a tolerant value: `#[serde(deserialize_with = "crate::tolerant::read")]`.
pub fn read<'de, D: Deserializer<'de>, T: Tolerant>(deserializer: D) -> Result<T, D::Error> {
    Ok(T::named(&String::deserialize(deserializer)?))
}

/// A field that may be absent or null: with `default` beside it.
pub fn read_option<'de, D: Deserializer<'de>, T: Tolerant>(deserializer: D) -> Result<Option<T>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.map(|word| T::named(&word)))
}
