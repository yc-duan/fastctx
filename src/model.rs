//! Protocol-neutral content model between the tool core and MCP layer.

/// Image-detail hint for the Codex vision pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageDetail {
    /// Request the high detail needed to preserve small PDF text.
    High,
}

/// One content block returned by a tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolContent {
    /// UTF-8 text for the model.
    Text(String),
    /// Base64 image data and MIME type; a missing detail uses the host default.
    Image {
        /// Standard Base64 data without a data-URL prefix.
        data: String,
        /// IANA MIME type corresponding to the magic bytes.
        mime_type: String,
        /// PDF page images request high detail; ordinary images defer to the host.
        detail: Option<ImageDetail>,
    },
}

/// Observable tool response; errors remain MCP content and are marked by is_error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResponse {
    /// Text or image blocks in model-consumption order.
    pub content: Vec<ToolContent>,
    /// When true, the MCP layer must return `isError: true`.
    pub is_error: bool,
}

impl ToolResponse {
    /// Returns one successful text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text(text.into())],
            is_error: false,
        }
    }

    /// Returns one text block with an explicit MCP error marker.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text(message.into())],
            is_error: true,
        }
    }
}
