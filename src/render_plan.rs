//! Immutable response units, exact head-first probes, and one-shot final rendering.

use crate::budget::estimate_tokens;
#[cfg(test)]
use crate::operation::TestStage;
use crate::operation::{WorkCheckpoint, WorkStop};
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
    Stopped(WorkStop),
    InvalidPrefix { shown: usize, available: usize },
    CountMismatch { probed: usize, full: usize },
    OverBudget { tokens: usize, budget: usize },
}

impl RenderPlanError {
    pub(crate) fn is_cancelled(&self) -> bool {
        matches!(self, Self::Stopped(WorkStop::RequestCancelled))
    }
}

impl fmt::Display for RenderPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped(WorkStop::RequestCancelled) => formatter.write_str("Request cancelled."),
            Self::Stopped(WorkStop::EpochRetired) => {
                formatter.write_str("The render generation was retired.")
            }
            Self::InvalidPrefix { shown, available } => write!(
                formatter,
                "The renderer selected {shown} entries from only {available} available entries."
            ),
            Self::CountMismatch { probed, full } => write!(
                formatter,
                "Internal token-count invariant failed: probed={probed}, full={full}."
            ),
            Self::OverBudget { tokens, budget } => write!(
                formatter,
                "The selected render uses {tokens} tokens but its budget is {budget}."
            ),
        }
    }
}

/// Lines prepared once with a byte boundary after every selectable prefix.
pub(crate) struct LineRenderGraph {
    body: String,
    prefix_ends: Vec<usize>,
}

impl LineRenderGraph {
    pub(crate) fn new(
        lines: Vec<Arc<str>>,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<Self, RenderPlanError> {
        let mut body = String::new();
        let mut prefix_ends = Vec::with_capacity(lines.len().saturating_add(1));
        prefix_ends.push(0);
        for (index, line) in lines.iter().enumerate() {
            check_render_work(operation, TestRenderStage::Unit)?;
            if index > 0 {
                body.push('\n');
            }
            body.push_str(line);
            prefix_ends.push(body.len());
        }

        Ok(Self { body, prefix_ends })
    }

    /// Exactly counts a leading head note, a body prefix, and trailing body details.
    pub(crate) fn probe_head<T: AsRef<str>>(
        &mut self,
        shown: usize,
        head: &str,
        details: &[T],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, RenderPlanError> {
        let body = self.body_prefix(shown)?;
        check_render_work(operation, TestRenderStage::TokenProbe)?;
        let text = render_head_first(head, body, details);
        count_rendered_tokens(&text, operation)
    }

    /// Assembles a head-first response once and independently verifies its exact token count.
    pub(crate) fn finish_head<T: AsRef<str>>(
        &mut self,
        shown: usize,
        head: &str,
        details: &[T],
        probed_tokens: usize,
        budget: usize,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<VerifiedRender, RenderPlanError> {
        let body = self.body_prefix(shown)?;
        check_render_work(operation, TestRenderStage::Unit)?;
        let text = render_head_first(head, body, details);
        check_render_work(operation, TestRenderStage::FinalVerify)?;
        let full_tokens = count_rendered_tokens(&text, operation)?;
        if full_tokens != probed_tokens {
            return Err(RenderPlanError::CountMismatch {
                probed: probed_tokens,
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

    fn body_prefix(&self, shown: usize) -> Result<Option<&str>, RenderPlanError> {
        let Some(end) = self.prefix_ends.get(shown).copied() else {
            return Err(RenderPlanError::InvalidPrefix {
                shown,
                available: self.prefix_ends.len().saturating_sub(1),
            });
        };
        Ok((shown > 0).then_some(&self.body[..end]))
    }
}

#[derive(Clone)]
pub(crate) struct LineRenderView {
    body: Arc<str>,
    line_count: usize,
}

/// Prepares multiple compatibility views that are not necessarily prefixes of one body.
pub(crate) struct SharedLineRenderGraph;

impl SharedLineRenderGraph {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Joins one immutable line view once for exact head-first probes.
    pub(crate) fn prepare_view(
        &mut self,
        lines: Vec<Arc<str>>,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<LineRenderView, RenderPlanError> {
        let line_count = lines.len();
        let mut body = String::new();
        for (depth, line) in lines.iter().enumerate() {
            check_render_work(operation, TestRenderStage::Unit)?;
            if depth > 0 {
                body.push('\n');
            }
            body.push_str(line);
        }
        Ok(LineRenderView {
            body: Arc::from(body),
            line_count,
        })
    }

    /// Exactly counts one prepared body view under a leading head note.
    pub(crate) fn probe_head<T: AsRef<str>>(
        &mut self,
        view: &LineRenderView,
        head: &str,
        details: &[T],
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<usize, RenderPlanError> {
        check_render_work(operation, TestRenderStage::TokenProbe)?;
        let text = render_head_first(head, view.body(), details);
        count_rendered_tokens(&text, operation)
    }

    /// Assembles one prepared body view below a head note and verifies the exact result.
    pub(crate) fn finish_head<T: AsRef<str>>(
        &mut self,
        view: &LineRenderView,
        head: &str,
        details: &[T],
        probed_tokens: usize,
        budget: usize,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<VerifiedRender, RenderPlanError> {
        check_render_work(operation, TestRenderStage::Unit)?;
        let text = render_head_first(head, view.body(), details);
        check_render_work(operation, TestRenderStage::FinalVerify)?;
        let full_tokens = count_rendered_tokens(&text, operation)?;
        if full_tokens != probed_tokens {
            return Err(RenderPlanError::CountMismatch {
                probed: probed_tokens,
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
}

impl LineRenderView {
    fn body(&self) -> Option<&str> {
        (self.line_count > 0).then_some(self.body.as_ref())
    }
}

fn render_head_first<T: AsRef<str>>(head: &str, body: Option<&str>, details: &[T]) -> String {
    let mut text = String::from(head);
    if let Some(body) = body {
        text.push('\n');
        text.push_str(body);
    }
    for detail in details {
        text.push('\n');
        text.push_str(detail.as_ref());
    }
    text
}

fn count_rendered_tokens(
    text: &str,
    operation: Option<&dyn WorkCheckpoint>,
) -> Result<usize, RenderPlanError> {
    let tokens = estimate_tokens(text);
    if let Some(operation) = operation {
        operation.check_work().map_err(RenderPlanError::Stopped)?;
    }
    Ok(tokens)
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
        operation.check_work().map_err(RenderPlanError::Stopped)?;
        #[cfg(test)]
        operation.stage(match stage {
            TestRenderStage::Unit => TestStage::RenderUnit,
            TestRenderStage::TokenProbe => TestStage::TokenProbe,
            TestRenderStage::FinalVerify => TestStage::BeforeFinalTokenVerify,
        });
        #[cfg(not(test))]
        let _ = stage;
        operation.check_work().map_err(RenderPlanError::Stopped)?;
    } else {
        let _ = stage;
    }
    Ok(())
}
