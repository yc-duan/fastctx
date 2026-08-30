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

/// Lines rendered exactly once, with an exact tokenizer checkpoint after every prefix.
pub(crate) struct LineRenderGraph {
    lines: Vec<Arc<str>>,
    checkpoints: Vec<TokenCheckpoint>,
    counter: ExactPrefixCounter,
}

impl LineRenderGraph {
    pub(crate) fn new(
        lines: Vec<Arc<str>>,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<Self, RenderPlanError> {
        let mut counter = ExactPrefixCounter::default();
        let mut checkpoints = Vec::with_capacity(lines.len().saturating_add(1));
        checkpoints.push(counter.checkpoint());
        for (index, line) in lines.iter().enumerate() {
            check_render_work(operation, TestRenderStage::Unit)?;
            if index > 0 {
                counter.append("\n", operation)?;
            }
            counter.append(line, operation)?;
            checkpoints.push(counter.checkpoint());
        }

        Ok(Self {
            lines,
            checkpoints,
            counter,
        })
    }

    /// Conservatively counts a leading head note, a body prefix, and trailing body details.
    pub(crate) fn probe_head<T: AsRef<str>>(
        &mut self,
        shown: usize,
        head: &str,
        details: &[T],
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
        let body_tokens = self.counter.count_with_suffix(checkpoint, "", operation)?;
        conservative_head_tokens(head, shown > 0, body_tokens, details)
    }

    /// Assembles a head-first response once and independently verifies its exact token count.
    pub(crate) fn finish_head<T: AsRef<str>>(
        &mut self,
        shown: usize,
        head: &str,
        details: &[T],
        conservative_tokens: usize,
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
        let text = render_head_first(head, &self.lines[..shown], details);
        check_render_work(operation, TestRenderStage::FinalVerify)?;
        let full_tokens = self.counter.verify_full(&text, operation)?;
        debug_assert!(full_tokens <= conservative_tokens);
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
}

#[derive(Clone)]
pub(crate) struct LineRenderView {
    lines: Arc<[Arc<str>]>,
    checkpoint: TokenCheckpoint,
}

struct SharedPrefixNode {
    checkpoint: TokenCheckpoint,
    children: HashMap<Arc<str>, usize>,
}

/// A request-local prefix trie for multiple compatibility views whose line
/// sequences overlap but are not necessarily prefixes of one maximum view.
pub(crate) struct SharedLineRenderGraph {
    nodes: Vec<SharedPrefixNode>,
}

impl SharedLineRenderGraph {
    pub(crate) fn new() -> Self {
        let counter = ExactPrefixCounter::default();
        Self {
            nodes: vec![SharedPrefixNode {
                checkpoint: counter.checkpoint(),
                children: HashMap::new(),
            }],
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
            }
            counter.append(line, operation)?;
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

    /// Conservatively counts one prepared body view under a leading head note.
    pub(crate) fn probe_head<T: AsRef<str>>(
        &mut self,
        view: &LineRenderView,
        head: &str,
        details: &[T],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, RenderPlanError> {
        check_render_work(operation, TestRenderStage::TokenProbe)?;
        let mut counter = ExactPrefixCounter::from_checkpoint(&view.checkpoint);
        let body_tokens = counter.count_with_suffix(&view.checkpoint, "", operation)?;
        conservative_head_tokens(head, !view.lines.is_empty(), body_tokens, details)
    }

    /// Assembles one prepared body view below a head note and verifies the exact result.
    pub(crate) fn finish_head<T: AsRef<str>>(
        &mut self,
        view: &LineRenderView,
        head: &str,
        details: &[T],
        conservative_tokens: usize,
        budget: usize,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<VerifiedRender, RenderPlanError> {
        check_render_work(operation, TestRenderStage::Unit)?;
        let text = render_head_first(head, &view.lines, details);
        check_render_work(operation, TestRenderStage::FinalVerify)?;
        let full_tokens = crate::budget::estimate_tokens(&text);
        check_render_work(operation, TestRenderStage::FinalVerify)?;
        debug_assert!(full_tokens <= conservative_tokens);
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
}

fn conservative_head_tokens<T: AsRef<str>>(
    head: &str,
    has_body: bool,
    body_tokens: usize,
    details: &[T],
) -> Result<usize, RenderPlanError> {
    let mut total = crate::budget::estimate_tokens(head);
    if has_body {
        total = total
            .checked_add(crate::budget::estimate_tokens("\n"))
            .and_then(|value| value.checked_add(body_tokens))
            .ok_or(RenderPlanError::Token(TokenCountError::Overflow))?;
    }
    for detail in details {
        total = total
            .checked_add(crate::budget::estimate_tokens("\n"))
            .and_then(|value| value.checked_add(crate::budget::estimate_tokens(detail.as_ref())))
            .ok_or(RenderPlanError::Token(TokenCountError::Overflow))?;
    }
    Ok(total)
}

fn render_head_first<T: AsRef<str>>(head: &str, body: &[Arc<str>], details: &[T]) -> String {
    let mut text = String::from(head);
    for line in body {
        text.push('\n');
        text.push_str(line);
    }
    for detail in details {
        text.push('\n');
        text.push_str(detail.as_ref());
    }
    text
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
