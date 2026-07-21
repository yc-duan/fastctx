//! Immutable response units, exact prefix checkpoints, and one-shot final rendering.

use crate::budget::{ExactPrefixCounter, TokenCheckpoint, TokenCountError};
#[cfg(test)]
use crate::operation::TestStage;
use crate::operation::{WorkCheckpoint, WorkStop};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A fully assembled response whose incremental and independent token counts agree.
#[derive(Debug)]
pub(crate) struct VerifiedRender {
    pub(crate) text: String,
    pub(crate) tokens: usize,
}

/// Failures that must stop output instead of risking a truncated response.
#[derive(Debug)]
pub(crate) enum RenderPlanError {
    Token(TokenCountError),
    InvalidPrefix { shown: usize, available: usize },
    InvalidTerminal,
    CountMismatch { incremental: usize, full: usize },
    OverBudget { tokens: usize, budget: usize },
}

impl RenderPlanError {
    pub(crate) fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Token(TokenCountError::Stopped(WorkStop::RequestCancelled))
        )
    }
}

impl fmt::Display for RenderPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(error) => error.fmt(formatter),
            Self::InvalidPrefix { shown, available } => write!(
                formatter,
                "The renderer selected {shown} entries from only {available} available entries."
            ),
            Self::InvalidTerminal => formatter.write_str(
                "The renderer received a terminal note outside the grep compatibility grammar.",
            ),
            Self::CountMismatch { incremental, full } => write!(
                formatter,
                "Internal token-count invariant failed: incremental={incremental}, full={full}."
            ),
            Self::OverBudget { tokens, budget } => write!(
                formatter,
                "The selected render uses {tokens} tokens but its budget is {budget}."
            ),
        }
    }
}

impl From<TokenCountError> for RenderPlanError {
    fn from(error: TokenCountError) -> Self {
        Self::Token(error)
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RenderPlanMetrics {
    pub(crate) render_units_built: usize,
    pub(crate) render_bytes_built: usize,
    pub(crate) token_prefix_appends: usize,
    pub(crate) token_suffix_probes: usize,
    pub(crate) full_tokenizer_calls: usize,
}

/// Lines rendered exactly once, with an exact tokenizer checkpoint after every prefix.
pub(crate) struct LineRenderGraph {
    lines: Vec<Arc<str>>,
    checkpoints: Vec<TokenCheckpoint>,
    counter: ExactPrefixCounter,
    #[cfg(test)]
    render_bytes_built: usize,
}

impl LineRenderGraph {
    pub(crate) fn new(
        lines: Vec<Arc<str>>,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<Self, RenderPlanError> {
        let mut counter = ExactPrefixCounter::default();
        let mut checkpoints = Vec::with_capacity(lines.len().saturating_add(1));
        checkpoints.push(counter.checkpoint());
        #[cfg(test)]
        let mut render_bytes_built = 0_usize;

        for (index, line) in lines.iter().enumerate() {
            check_render_work(operation, TestRenderStage::Unit)?;
            if index > 0 {
                counter.append("\n", operation)?;
            }
            counter.append(line, operation)?;
            checkpoints.push(counter.checkpoint());
            #[cfg(test)]
            {
                render_bytes_built = render_bytes_built.saturating_add(line.len());
            }
        }

        Ok(Self {
            lines,
            checkpoints,
            counter,
            #[cfg(test)]
            render_bytes_built,
        })
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lines.len()
    }

    /// Returns the immutable tokenizer state at one body-entry prefix.
    pub(crate) fn checkpoint(&self, shown: usize) -> Result<TokenCheckpoint, RenderPlanError> {
        self.checkpoints
            .get(shown)
            .cloned()
            .ok_or(RenderPlanError::InvalidPrefix {
                shown,
                available: self.lines.len(),
            })
    }

    /// Counts a prefix plus its notes using only the checkpoint tail and short trailer.
    pub(crate) fn probe_notes<T: AsRef<str>>(
        &mut self,
        shown: usize,
        notes: &[T],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, RenderPlanError> {
        check_render_work(operation, TestRenderStage::TokenProbe)?;
        let checkpoint = self
            .checkpoints
            .get(shown)
            .ok_or(RenderPlanError::InvalidPrefix {
                shown,
                available: self.lines.len(),
            })?;
        let trailer = render_notes_suffix(shown, notes);
        self.counter
            .count_with_suffix(checkpoint, &trailer, operation)
            .map_err(Into::into)
    }

    /// Assembles the selected view once, then independently verifies the full text once.
    pub(crate) fn finish<T: AsRef<str>>(
        &mut self,
        shown: usize,
        notes: &[T],
        incremental_tokens: usize,
        budget: usize,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<VerifiedRender, RenderPlanError> {
        if shown > self.lines.len() {
            return Err(RenderPlanError::InvalidPrefix {
                shown,
                available: self.lines.len(),
            });
        }
        check_render_work(operation, TestRenderStage::Unit)?;
        let mut text = String::new();
        for (index, line) in self.lines[..shown].iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            text.push_str(line);
        }
        if !notes.is_empty() {
            if shown > 0 {
                text.push_str("\n\n");
            }
            for (index, note) in notes.iter().enumerate() {
                if index > 0 {
                    text.push('\n');
                }
                text.push_str(note.as_ref());
            }
        }

        check_render_work(operation, TestRenderStage::FinalVerify)?;
        let full_tokens = self.counter.verify_full(&text, operation)?;
        if full_tokens != incremental_tokens {
            return Err(RenderPlanError::CountMismatch {
                incremental: incremental_tokens,
                full: full_tokens,
            });
        }
        if full_tokens > budget {
            return Err(RenderPlanError::OverBudget {
                tokens: full_tokens,
                budget,
            });
        }
        Ok(VerifiedRender {
            text,
            tokens: full_tokens,
        })
    }

    #[cfg(test)]
    pub(crate) fn metrics(&self) -> RenderPlanMetrics {
        let token = self.counter.metrics();
        RenderPlanMetrics {
            render_units_built: self.lines.len(),
            render_bytes_built: self.render_bytes_built,
            token_prefix_appends: token.prefix_appends,
            token_suffix_probes: token.suffix_probes,
            full_tokenizer_calls: token.full_tokenizer_calls,
        }
    }
}

#[derive(Clone)]
pub(crate) struct LineRenderView {
    lines: Arc<[Arc<str>]>,
    checkpoint: TokenCheckpoint,
}

impl LineRenderView {
    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn checkpoint(&self) -> &TokenCheckpoint {
        &self.checkpoint
    }
}

struct SharedPrefixNode {
    checkpoint: TokenCheckpoint,
    children: HashMap<Arc<str>, usize>,
}

/// A request-local prefix trie for multiple compatibility views whose line
/// sequences overlap but are not necessarily prefixes of one maximum view.
pub(crate) struct SharedLineRenderGraph {
    nodes: Vec<SharedPrefixNode>,
    #[cfg(test)]
    token_prefix_appends: usize,
    #[cfg(test)]
    token_suffix_probes: usize,
    #[cfg(test)]
    full_tokenizer_calls: usize,
    #[cfg(test)]
    render_bytes_built: usize,
}

impl SharedLineRenderGraph {
    pub(crate) fn new() -> Self {
        let counter = ExactPrefixCounter::default();
        Self {
            nodes: vec![SharedPrefixNode {
                checkpoint: counter.checkpoint(),
                children: HashMap::new(),
            }],
            #[cfg(test)]
            token_prefix_appends: 0,
            #[cfg(test)]
            token_suffix_probes: 0,
            #[cfg(test)]
            full_tokenizer_calls: 0,
            #[cfg(test)]
            render_bytes_built: 0,
        }
    }

    /// Interns one immutable line view, tokenizing only prefix edges that no
    /// earlier compatibility probe has already established.
    pub(crate) fn prepare_view(
        &mut self,
        lines: Vec<Arc<str>>,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<LineRenderView, RenderPlanError> {
        let mut node_index = 0_usize;
        for (depth, line) in lines.iter().enumerate() {
            check_render_work(operation, TestRenderStage::Unit)?;
            if let Some(child) = self.nodes[node_index].children.get(line).copied() {
                node_index = child;
                continue;
            }

            let parent_checkpoint = self.nodes[node_index].checkpoint.clone();
            let mut counter = ExactPrefixCounter::from_checkpoint(&parent_checkpoint);
            if depth > 0 {
                counter.append("\n", operation)?;
                #[cfg(test)]
                {
                    self.token_prefix_appends = self.token_prefix_appends.saturating_add(1);
                }
            }
            counter.append(line, operation)?;
            #[cfg(test)]
            {
                self.token_prefix_appends = self.token_prefix_appends.saturating_add(1);
                self.render_bytes_built = self.render_bytes_built.saturating_add(line.len());
            }
            let child = self.nodes.len();
            self.nodes.push(SharedPrefixNode {
                checkpoint: counter.checkpoint(),
                children: HashMap::new(),
            });
            self.nodes[node_index]
                .children
                .insert(Arc::clone(line), child);
            node_index = child;
        }
        Ok(LineRenderView {
            lines: Arc::from(lines),
            checkpoint: self.nodes[node_index].checkpoint.clone(),
        })
    }

    pub(crate) fn probe_notes<T: AsRef<str>>(
        &mut self,
        view: &LineRenderView,
        notes: &[T],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, RenderPlanError> {
        check_render_work(operation, TestRenderStage::TokenProbe)?;
        #[cfg(test)]
        {
            self.token_suffix_probes = self.token_suffix_probes.saturating_add(1);
        }
        let suffix = render_notes_suffix(view.len(), notes);
        let mut counter = ExactPrefixCounter::from_checkpoint(&view.checkpoint);
        counter
            .count_with_suffix(&view.checkpoint, &suffix, operation)
            .map_err(Into::into)
    }

    pub(crate) fn finish<T: AsRef<str>>(
        &mut self,
        view: &LineRenderView,
        notes: &[T],
        incremental_tokens: usize,
        budget: usize,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<VerifiedRender, RenderPlanError> {
        check_render_work(operation, TestRenderStage::Unit)?;
        let mut text = String::new();
        for (index, line) in view.lines.iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            text.push_str(line);
        }
        if !notes.is_empty() {
            if !view.lines.is_empty() {
                text.push_str("\n\n");
            }
            for (index, note) in notes.iter().enumerate() {
                if index > 0 {
                    text.push('\n');
                }
                text.push_str(note.as_ref());
            }
        }

        check_render_work(operation, TestRenderStage::FinalVerify)?;
        #[cfg(test)]
        {
            self.full_tokenizer_calls = self.full_tokenizer_calls.saturating_add(1);
        }
        let full_tokens = crate::budget::estimate_tokens(&text);
        check_render_work(operation, TestRenderStage::FinalVerify)?;
        if full_tokens != incremental_tokens {
            return Err(RenderPlanError::CountMismatch {
                incremental: incremental_tokens,
                full: full_tokens,
            });
        }
        if full_tokens > budget {
            return Err(RenderPlanError::OverBudget {
                tokens: full_tokens,
                budget,
            });
        }
        Ok(VerifiedRender {
            text,
            tokens: full_tokens,
        })
    }

    #[cfg(test)]
    pub(crate) fn metrics(&self) -> RenderPlanMetrics {
        RenderPlanMetrics {
            render_units_built: self.nodes.len().saturating_sub(1),
            render_bytes_built: self.render_bytes_built,
            token_prefix_appends: self.token_prefix_appends,
            token_suffix_probes: self.token_suffix_probes,
            full_tokenizer_calls: self.full_tokenizer_calls,
        }
    }
}

/// Exact checkpoints for an optional prefix of diagnostic detail lines after a fixed body.
pub(crate) struct DetailRenderGraph {
    prefix_has_body: bool,
    fixed_lines: usize,
    detail_lines: usize,
    checkpoints: Vec<TokenCheckpoint>,
    counter: ExactPrefixCounter,
    #[cfg(test)]
    render_bytes_built: usize,
}

impl DetailRenderGraph {
    pub(crate) fn new(
        body_checkpoint: &TokenCheckpoint,
        prefix_has_body: bool,
        fixed_lines: &[Arc<str>],
        detail_lines: &[Arc<str>],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<Self, RenderPlanError> {
        let mut counter = ExactPrefixCounter::from_checkpoint(body_checkpoint);
        let mut note_count = 0_usize;
        for line in fixed_lines {
            append_note_line(&mut counter, line, prefix_has_body, note_count, operation)?;
            note_count = note_count.saturating_add(1);
        }
        let mut checkpoints = Vec::with_capacity(detail_lines.len().saturating_add(1));
        checkpoints.push(counter.checkpoint());
        for line in detail_lines {
            append_note_line(&mut counter, line, prefix_has_body, note_count, operation)?;
            note_count = note_count.saturating_add(1);
            checkpoints.push(counter.checkpoint());
        }
        Ok(Self {
            prefix_has_body,
            fixed_lines: fixed_lines.len(),
            detail_lines: detail_lines.len(),
            checkpoints,
            counter,
            #[cfg(test)]
            render_bytes_built: fixed_lines
                .iter()
                .chain(detail_lines)
                .map(|line| line.len())
                .sum(),
        })
    }

    /// Counts the selected detail prefix plus mandatory trailing note lines.
    pub(crate) fn probe_tail<T: AsRef<str>>(
        &mut self,
        shown_details: usize,
        tail_lines: &[T],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, RenderPlanError> {
        check_render_work(operation, TestRenderStage::TokenProbe)?;
        let checkpoint =
            self.checkpoints
                .get(shown_details)
                .ok_or(RenderPlanError::InvalidPrefix {
                    shown: shown_details,
                    available: self.detail_lines,
                })?;
        let existing_notes = self.fixed_lines.saturating_add(shown_details);
        let suffix = render_continuation_suffix(self.prefix_has_body, existing_notes, tail_lines);
        self.counter
            .count_with_suffix(checkpoint, &suffix, operation)
            .map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn metrics(&self) -> RenderPlanMetrics {
        let token = self.counter.metrics();
        RenderPlanMetrics {
            render_units_built: self.fixed_lines.saturating_add(self.detail_lines),
            render_bytes_built: self.render_bytes_built,
            token_prefix_appends: token.prefix_appends,
            token_suffix_probes: token.suffix_probes,
            full_tokenizer_calls: token.full_tokenizer_calls,
        }
    }
}

fn append_note_line(
    counter: &mut ExactPrefixCounter,
    line: &str,
    prefix_has_body: bool,
    existing_notes: usize,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<(), RenderPlanError> {
    if existing_notes > 0 {
        counter.append("\n", operation)?;
    } else if prefix_has_body {
        counter.append("\n\n", operation)?;
    }
    counter.append(line, operation)?;
    Ok(())
}

fn render_continuation_suffix<T: AsRef<str>>(
    prefix_has_body: bool,
    existing_notes: usize,
    lines: &[T],
) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut suffix = String::new();
    if existing_notes > 0 {
        suffix.push('\n');
    } else if prefix_has_body {
        suffix.push_str("\n\n");
    }
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            suffix.push('\n');
        }
        suffix.push_str(line.as_ref());
    }
    suffix
}

fn render_notes_suffix<T: AsRef<str>>(shown: usize, notes: &[T]) -> String {
    if notes.is_empty() {
        return String::new();
    }
    let mut suffix = String::new();
    if shown > 0 {
        suffix.push_str("\n\n");
    }
    for (index, note) in notes.iter().enumerate() {
        if index > 0 {
            suffix.push('\n');
        }
        suffix.push_str(note.as_ref());
    }
    suffix
}

#[derive(Clone, Copy)]
enum TestRenderStage {
    Unit,
    TokenProbe,
    FinalVerify,
}

fn check_render_work(
    operation: Option<&dyn WorkCheckpoint>,
    stage: TestRenderStage,
) -> Result<(), RenderPlanError> {
    if let Some(operation) = operation {
        operation.check_work().map_err(TokenCountError::Stopped)?;
        #[cfg(test)]
        operation.stage(match stage {
            TestRenderStage::Unit => TestStage::RenderUnit,
            TestRenderStage::TokenProbe => TestStage::TokenProbe,
            TestRenderStage::FinalVerify => TestStage::BeforeFinalTokenVerify,
        });
        #[cfg(not(test))]
        let _ = stage;
        operation.check_work().map_err(TokenCountError::Stopped)?;
    } else {
        let _ = stage;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DetailRenderGraph, LineRenderGraph, RenderPlanError, SharedLineRenderGraph};
    use crate::budget::estimate_tokens;
    use crate::operation::{RequestWorkGuard, TestStage};
    use rmcp::model::RequestId;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn every_prefix_probe_equals_an_independent_full_count() {
        let lines = ["alpha", "界 123", "/// punctuation", "尾"]
            .into_iter()
            .map(Arc::<str>::from)
            .collect();
        let mut graph = LineRenderGraph::new(lines, None).unwrap();

        for shown in 0..=graph.len() {
            let terminal = format!("(Complete: {shown} shown.)");
            let notes = [terminal];
            let incremental = graph.probe_notes(shown, &notes, None).unwrap();
            let expected = if shown == 0 {
                notes[0].clone()
            } else {
                format!(
                    "{}\n\n{}",
                    ["alpha", "界 123", "/// punctuation", "尾"][..shown].join("\n"),
                    notes[0]
                )
            };
            assert_eq!(incremental, estimate_tokens(&expected));
        }
    }

    #[test]
    fn final_render_is_assembled_and_full_tokenized_once() {
        let lines = (0..1_000)
            .map(|index| Arc::<str>::from(format!("/path/{index:04}.txt")))
            .collect();
        let mut graph = LineRenderGraph::new(lines, None).unwrap();
        let notes = ["(Complete: all 1000 files shown.)".to_string()];
        let incremental = graph.probe_notes(1_000, &notes, None).unwrap();
        let rendered = graph
            .finish(1_000, &notes, incremental, usize::MAX, None)
            .unwrap();

        assert_eq!(rendered.tokens, estimate_tokens(&rendered.text));
        let metrics = graph.metrics();
        assert_eq!(metrics.render_units_built, 1_000);
        assert_eq!(metrics.full_tokenizer_calls, 1);
        assert_eq!(metrics.token_suffix_probes, 1);
        assert!(metrics.token_prefix_appends <= 2_000);
    }

    #[test]
    fn detail_prefixes_match_independent_counts_with_and_without_a_body() {
        for (body, fixed, details) in [
            (
                vec![Arc::<str>::from("body")],
                vec![Arc::<str>::from("fixed")],
                vec![
                    Arc::<str>::from("detail one"),
                    Arc::<str>::from("detail two"),
                ],
            ),
            (
                Vec::new(),
                Vec::new(),
                vec![
                    Arc::<str>::from("detail one"),
                    Arc::<str>::from("detail two"),
                ],
            ),
        ] {
            let body_len = body.len();
            let mut body_graph = LineRenderGraph::new(body.clone(), None).unwrap();
            let checkpoint = body_graph.checkpoint(body_len).unwrap();
            let mut details_graph =
                DetailRenderGraph::new(&checkpoint, body_len > 0, &fixed, &details, None).unwrap();
            for shown in 0..=details.len() {
                let tail = [Arc::<str>::from(format!("terminal {shown}"))];
                let actual = details_graph.probe_tail(shown, &tail, None).unwrap();
                let mut notes = fixed.iter().map(AsRef::as_ref).collect::<Vec<_>>();
                notes.extend(details[..shown].iter().map(AsRef::as_ref));
                notes.push(tail[0].as_ref());
                let expected = if body.is_empty() {
                    notes.join("\n")
                } else {
                    format!(
                        "{}\n\n{}",
                        body.iter()
                            .map(AsRef::as_ref)
                            .collect::<Vec<_>>()
                            .join("\n"),
                        notes.join("\n")
                    )
                };
                assert_eq!(actual, estimate_tokens(&expected));
            }

            let terminal = [Arc::<str>::from("terminal")];
            let incremental = body_graph.probe_notes(body_len, &terminal, None).unwrap();
            let error = body_graph
                .finish(body_len, &terminal, incremental, 0, None)
                .unwrap_err();
            assert!(matches!(error, RenderPlanError::OverBudget { .. }));
        }
    }

    #[test]
    fn shared_views_reuse_prefix_edges_and_verify_only_the_selected_view() {
        let mut graph = SharedLineRenderGraph::new();
        let first = graph
            .prepare_view(
                ["a", "b", "context"]
                    .into_iter()
                    .map(Arc::<str>::from)
                    .collect(),
                None,
            )
            .unwrap();
        let second = graph
            .prepare_view(
                ["a", "b", "match", "tail"]
                    .into_iter()
                    .map(Arc::<str>::from)
                    .collect(),
                None,
            )
            .unwrap();
        let notes = [Arc::<str>::from("(Complete.)")];
        let tokens = graph.probe_notes(&second, &notes, None).unwrap();
        let rendered = graph
            .finish(&second, &notes, tokens, usize::MAX, None)
            .unwrap();
        assert_eq!(rendered.text, "a\nb\nmatch\ntail\n\n(Complete.)");
        assert_eq!(rendered.tokens, estimate_tokens(&rendered.text));
        let metrics = graph.metrics();
        assert_eq!(metrics.render_units_built, 5);
        assert_eq!(metrics.full_tokenizer_calls, 1);
        assert!(metrics.token_prefix_appends <= 10);
        assert_eq!(first.len(), 3);
    }

    #[test]
    fn diagnostic_detail_graph_is_linear_for_large_skip_reports() {
        for count in [1_usize, 10, 10_000] {
            let body = LineRenderGraph::new(vec![Arc::<str>::from("body")], None).unwrap();
            let checkpoint = body.checkpoint(1).unwrap();
            let details = (0..count)
                .map(|index| {
                    let reason = if index % 2 == 0 {
                        "mixed or inconsistent encodings"
                    } else {
                        "changed while being searched"
                    };
                    Arc::<str>::from(format!("/skip/{index:05} — {reason}"))
                })
                .collect::<Vec<_>>();
            let expected_bytes = details.iter().map(|line| line.len()).sum::<usize>();
            let mut graph = DetailRenderGraph::new(&checkpoint, true, &[], &details, None).unwrap();
            let tail = [Arc::<str>::from(format!(
                "(Complete: all 1 result shown; {count} files skipped.)"
            ))];
            let tokens = graph.probe_tail(count, &tail, None).unwrap();
            assert!(tokens > 0);
            let metrics = graph.metrics();
            assert_eq!(metrics.render_units_built, count);
            assert_eq!(metrics.render_bytes_built, expected_bytes);
            assert!(metrics.token_prefix_appends <= count.saturating_mul(2));
            assert_eq!(metrics.token_suffix_probes, 1);
            assert_eq!(metrics.full_tokenizer_calls, 0);
        }
    }

    #[test]
    fn render_and_token_stages_surface_cancellation_without_a_success_body() {
        for cancelled_stage in [
            TestStage::RenderUnit,
            TestStage::TokenProbe,
            TestStage::BeforeFinalTokenVerify,
        ] {
            let parent = CancellationToken::new();
            let cancel_from_hook = parent.clone();
            let hook = Arc::new(move |stage| {
                if stage == cancelled_stage {
                    cancel_from_hook.cancel();
                }
            });
            let (mut guard, operation) = RequestWorkGuard::new_with_hook(
                RequestId::String(Arc::from(format!("render-{cancelled_stage:?}"))),
                parent,
                hook,
            );
            let lines = vec![Arc::<str>::from("body")];
            let notes = [Arc::<str>::from("(Complete.)")];
            let result = match cancelled_stage {
                TestStage::RenderUnit => LineRenderGraph::new(lines, Some(&operation)).map(|_| ()),
                TestStage::TokenProbe => {
                    let mut graph = LineRenderGraph::new(lines, Some(&operation)).unwrap();
                    graph.probe_notes(1, &notes, Some(&operation)).map(|_| ())
                }
                TestStage::BeforeFinalTokenVerify => {
                    let mut graph = LineRenderGraph::new(lines, Some(&operation)).unwrap();
                    let tokens = graph.probe_notes(1, &notes, Some(&operation)).unwrap();
                    graph
                        .finish(1, &notes, tokens, usize::MAX, Some(&operation))
                        .map(|_| ())
                }
                _ => unreachable!(),
            };
            let error = result.unwrap_err();
            assert!(error.is_cancelled(), "stage={cancelled_stage:?}: {error}");
            guard.disarm();
        }
    }
}
