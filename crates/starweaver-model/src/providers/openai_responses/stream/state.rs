//! Streamed `OpenAI` Responses state.

/// Incrementally assembled function-call item from `OpenAI` Responses streaming events.
#[derive(Clone, Debug, Default)]
pub(super) struct StreamedFunctionCall {
    pub(super) index: usize,
    pub(super) item_id: String,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: String,
    pub(super) namespace: Option<String>,
    pub(super) status: Option<String>,
    pub(super) started: bool,
    pub(super) ended: bool,
}
