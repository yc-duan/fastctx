//! Structured leading coordinates for successful model-visible responses.

use crate::model::ToolResponse;

/// A one-based inclusive range emitted in a response body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CoveredRange {
    first: usize,
    last: usize,
}

impl CoveredRange {
    /// Creates a non-empty inclusive range.
    pub(crate) fn new(first: usize, last: usize) -> Self {
        debug_assert!(first > 0 && last >= first);
        Self { first, last }
    }

    /// Reports whether this range exactly matches the supplied bounds.
    pub(crate) const fn is(&self, first: usize, last: usize) -> bool {
        self.first == first && self.last == last
    }
}

/// The honest scale available for one coverage metric.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoverageTotal {
    /// The complete set has this exact size.
    Exact(usize),
    /// Scanning stopped after proving at least this many entries exist.
    AtLeast(usize),
    /// A text file was too large to count completely, but its exact byte size is known.
    FileBytes(u64),
}

/// A closed metric family shared by every successful response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HeadMetric {
    /// One or more body ranges measured against an exact or honest total.
    Coverage {
        /// Stable plural unit, such as `lines`, `pages`, `matches`, or `jobs`.
        unit: &'static str,
        /// Ranges actually emitted by FastCtx, in display order.
        ranges: Vec<CoveredRange>,
        /// Available total scale.
        total: CoverageTotal,
    },
    /// An exact count, including zero-result responses.
    Count {
        /// Number of items.
        count: usize,
        /// Singular noun.
        singular: &'static str,
        /// Plural noun.
        plural: &'static str,
    },
    /// An exact operation count qualified by the number of affected files.
    CountInFiles {
        /// Primary count.
        count: usize,
        /// Singular primary noun.
        singular: &'static str,
        /// Plural primary noun.
        plural: &'static str,
        /// Number of files represented by the primary count.
        files: usize,
    },
    /// A bodyless lifecycle event whose complete response fits in the head note itself.
    Event(String),
}

impl HeadMetric {
    /// Builds one exact count metric.
    pub(crate) const fn count(count: usize, singular: &'static str, plural: &'static str) -> Self {
        Self::Count {
            count,
            singular,
            plural,
        }
    }

    /// Builds one exact primary/file count metric.
    pub(crate) const fn count_in_files(
        count: usize,
        singular: &'static str,
        plural: &'static str,
        files: usize,
    ) -> Self {
        Self::CountInFiles {
            count,
            singular,
            plural,
            files,
        }
    }

    /// Builds a lifecycle event metric.
    pub(crate) fn event(event: impl Into<String>) -> Self {
        Self::Event(event.into())
    }

    fn render(&self) -> String {
        match self {
            Self::Coverage {
                unit,
                ranges,
                total,
            } => {
                let covered = ranges
                    .iter()
                    .map(|range| {
                        if range.first == range.last {
                            range.first.to_string()
                        } else {
                            format!("{}-{}", range.first, range.last)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" and ");
                match total {
                    CoverageTotal::Exact(total) => format!("{unit} {covered} of {total}"),
                    CoverageTotal::AtLeast(total) => {
                        format!("{unit} {covered} of at least {total}")
                    }
                    CoverageTotal::FileBytes(bytes) => {
                        format!("{unit} {covered} of a {bytes}-byte file")
                    }
                }
            }
            Self::Count {
                count,
                singular,
                plural,
            } => format!("{count} {}", noun(*count, singular, plural)),
            Self::CountInFiles {
                count,
                singular,
                plural,
                files,
            } => format!(
                "{count} {} in {files} {}",
                noun(*count, singular, plural),
                noun(*files, "file", "files")
            ),
            Self::Event(event) => single_line(event),
        }
    }
}

/// The leading fact envelope for one successful response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HeadNote {
    subject: String,
    metric: HeadMetric,
    facts: Vec<String>,
}

impl HeadNote {
    /// Creates a note for a model-visible subject and metric.
    pub(crate) fn new(subject: impl Into<String>, metric: HeadMetric) -> Self {
        Self {
            subject: subject.into(),
            metric,
            facts: Vec::new(),
        }
    }

    /// Appends one concise fact known only to FastCtx.
    pub(crate) fn fact(mut self, fact: impl Into<String>) -> Self {
        self.facts.push(fact.into());
        self
    }

    /// Renders the stable one-line envelope.
    pub(crate) fn render(&self) -> String {
        let mut clauses = vec![self.metric.render()];
        clauses.extend(self.facts.iter().map(|fact| single_line(fact)));
        format!(
            "=== {} ({}) ===",
            single_line(&self.subject),
            clauses.join("; ")
        )
    }

    /// Prepends the note to an optional text body.
    pub(crate) fn render_with_body(&self, body: &str) -> String {
        if body.is_empty() {
            self.render()
        } else {
            format!("{}\n{}", self.render(), body)
        }
    }

    /// Returns a successful text response with this note at the head.
    pub(crate) fn into_text_response(self, body: &str) -> ToolResponse {
        ToolResponse::text(self.render_with_body(body))
    }
}

fn noun<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn single_line(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}
