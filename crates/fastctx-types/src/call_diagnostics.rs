//! Diagnostic lifetimes are separate from scheduling pools and transport streams.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// An observation or lease kept until its reader leaves, including long polling.
    Subscription,
    /// A single answer with a total deadline, even when the work may take minutes.
    Request,
    /// A job, transfer or service session governed by its progress and connection.
    Operation,
    /// The envelope has not exposed its call, or an older peer omitted the class.
    #[default]
    #[serde(other)]
    Unknown,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Self::Subscription => "subscriptions",
            Self::Request => "requests",
            Self::Operation => "operations",
            Self::Unknown => "unknown",
        }
    }
}

impl crate::remote::RemoteCall {
    pub fn diagnostic_class(&self) -> Class {
        use crate::{file_search::FileSearchRequest, lines::Request as Lines, remote::RemoteCall};
        match self {
            RemoteCall::Timeline { request: crate::timeline_pages::TimelineAsk::Moved { .. } }
            | RemoteCall::Watch { .. }
            | RemoteCall::Lines { request: Lines::Open { .. } | Lines::Changed { .. } }
            | RemoteCall::FindFiles { request: FileSearchRequest::Open { .. } } => Class::Subscription,
            _ if self.contract().within.is_some() => Class::Request,
            _ => Class::Operation,
        }
    }
}

/// Ages of calls still open at sampling time, not durations of completed calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ages {
    pub count: u64,
    pub oldest_seconds: u64,
    /// Counts in [0,60), [60,300), [300,3600), [3600,86400), [86400,infinity).
    pub buckets: [u64; 5],
    /// Only a declared total deadline can make a call overdue.
    pub overdue: u64,
}

impl Ages {
    pub fn record(&mut self, seconds: u64, overdue: bool) {
        self.count += 1;
        self.oldest_seconds = self.oldest_seconds.max(seconds);
        let bucket = [60, 300, 3600, 86400].iter().position(|bound| seconds < *bound).unwrap_or(4);
        self.buckets[bucket] += 1;
        self.overdue += u64::from(overdue);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub subscriptions: Ages,
    pub requests: Ages,
    pub operations: Ages,
    pub unknown: Ages,
}

impl Summary {
    pub fn record(&mut self, class: Class, seconds: u64, overdue: bool) {
        match class {
            Class::Subscription => &mut self.subscriptions,
            Class::Request => &mut self.requests,
            Class::Operation => &mut self.operations,
            Class::Unknown => &mut self.unknown,
        }
        .record(seconds, overdue);
    }

    pub fn groups(self) -> [(Class, Ages); 4] {
        [
            (Class::Subscription, self.subscriptions),
            (Class::Request, self.requests),
            (Class::Operation, self.operations),
            (Class::Unknown, self.unknown),
        ]
    }
}
