//! Closed local index families writable only inside the registered source writer.
use super::DataClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFamily {
    Aliases,
    HostRows,
    Consumers,
}
impl SourceFamily {
    pub const ALL: [Self; 3] = [Self::Aliases, Self::HostRows, Self::Consumers];
    pub fn id(self) -> &'static str {
        match self {
            Self::Aliases => "source.aliases",
            Self::HostRows => "source.host-rows",
            Self::Consumers => "source.consumers",
        }
    }
    pub fn class(self) -> DataClass {
        match self {
            Self::Aliases | Self::HostRows => DataClass::Derived,
            Self::Consumers => DataClass::Runtime,
        }
    }
    pub fn owner_kinds(self) -> &'static [&'static str] {
        match self {
            Self::Aliases | Self::HostRows | Self::Consumers => {
                &["conversation.timeline", "conversation.timeline-observation", "conversation.agents"]
            }
        }
    }
}
