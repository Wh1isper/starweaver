//! Model-visible tool for asking the user clarifying questions.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_tools::{
    DynToolset, StaticToolset, Tool, ToolApprovalState, ToolContext, ToolError, ToolResult,
    ToolUserInputPreprocessResult,
};

/// Stable model-visible tool name.
pub const ASK_USER_QUESTION_TOOL_NAME: &str = "ask_user_question";
/// Approval request kind used by hosts to render clarifying questions.
pub const CLARIFYING_QUESTIONS_REQUEST_KIND: &str = "clarifying_questions";
/// Approval metadata key containing normalized user answers.
pub const CLARIFYING_ANSWERS_METADATA_KEY: &str = "clarifying_answers";

const MAX_QUESTIONS: usize = 4;
const MAX_HEADER_CHARS: usize = 12;
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 4;
const MAX_RESPONSE_CHARS: usize = 16_384;

/// One selectable answer for a clarifying question.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarifyingQuestionOption {
    /// Concise option label shown to the user.
    #[schemars(length(min = 1))]
    pub label: String,
    /// Explanation of what choosing this option means.
    #[schemars(length(min = 1))]
    pub description: String,
    /// Optional longer preview shown by capable hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// One question presented to the user.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarifyingQuestion {
    /// Complete question text.
    #[schemars(length(min = 1))]
    pub question: String,
    /// Short UI header of at most 12 characters.
    #[schemars(length(min = 1, max = 12))]
    pub header: String,
    /// Two to four suggested answers.
    #[schemars(length(min = 2, max = 4))]
    pub options: Vec<ClarifyingQuestionOption>,
    /// Whether the user may select multiple options.
    pub multi_select: bool,
}

/// Arguments accepted by [`ASK_USER_QUESTION_TOOL_NAME`].
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUserQuestionArgs {
    /// One to four clarifying questions.
    #[schemars(length(min = 1, max = 4))]
    pub questions: Vec<ClarifyingQuestion>,
}

/// User answers supplied by a host while resolving the clarifying request.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarifyingQuestionAnswers {
    /// Answers keyed by the exact question text.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub answers: BTreeMap<String, String>,
    /// Optional free-form response applying to the request as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 16384))]
    pub response: Option<String>,
}

/// Successful result returned to the model after the user responds.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUserQuestionResult {
    /// Original questions asked by the model.
    pub questions: Vec<ClarifyingQuestion>,
    /// Answers keyed by exact question text.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub answers: BTreeMap<String, String>,
    /// Optional free-form response applying to the request as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 16384))]
    pub response: Option<String>,
}

struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        ASK_USER_QUESTION_TOOL_NAME
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Ask the user one to four clarifying questions before continuing. Call this tool by itself when missing information would materially affect the result.",
        )
    }

    fn parameters_schema(&self) -> Value {
        match serde_json::to_value(schemars::schema_for!(AskUserQuestionArgs)) {
            Ok(schema) => schema,
            Err(error) => serde_json::json!({
                "type": "object",
                "description": format!("failed to serialize ask-user-question schema: {error}"),
            }),
        }
    }

    fn metadata(&self) -> Metadata {
        Metadata::from_iter([(
            starweaver_tools::TOOL_METADATA_SELF_MANAGED_HITL_KEY.to_string(),
            Value::Bool(true),
        )])
    }

    fn return_schema(&self) -> Option<Value> {
        Some(
            serde_json::to_value(schemars::schema_for!(AskUserQuestionResult)).unwrap_or_else(
                |error| {
                    serde_json::json!({
                        "type": "object",
                        "description": format!("failed to serialize ask-user-question result schema: {error}"),
                    })
                },
            ),
        )
    }

    fn sequential(&self) -> Option<bool> {
        Some(true)
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        let arguments = parse_arguments(arguments)?;
        validate_questions(&arguments.questions)?;

        match context.approval {
            None => Err(ToolError::ApprovalRequired {
                tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
                metadata: serde_json::json!({
                    "kind": CLARIFYING_QUESTIONS_REQUEST_KIND,
                    "questions": arguments.questions,
                }),
            }),
            Some(ToolApprovalState::Approved { metadata, .. }) => {
                let answers = metadata
                    .get(CLARIFYING_ANSWERS_METADATA_KEY)
                    .cloned()
                    .ok_or_else(|| ToolError::UserError {
                        tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
                        message: "approved clarifying request did not include user answers"
                            .to_string(),
                    })?;
                let answers = parse_answers(answers)?;
                let questions = serialize_tool_value(&arguments.questions)?;
                let answers = serialize_tool_value(answers)?;
                let result = resolve_clarifying_question_answers(questions, answers)?;
                Ok(ToolResult::new(serialize_tool_value(result)?))
            }
            Some(ToolApprovalState::Denied { reason, .. }) => Err(ToolError::UserError {
                tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
                message: reason.unwrap_or_else(|| "the user declined to answer".to_string()),
            }),
        }
    }

    async fn preprocess_user_input(
        &self,
        _context: ToolContext,
        user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        let answers = normalize_clarifying_question_answers(user_input)?;
        validate_answer_payload(&answers)?;
        let mut metadata = Metadata::default();
        metadata.insert(
            CLARIFYING_ANSWERS_METADATA_KEY.to_string(),
            serialize_tool_value(answers)?,
        );
        Ok(ToolUserInputPreprocessResult::new().with_metadata(metadata))
    }
}

/// Create the first-party user-input toolset.
#[must_use]
pub fn user_input_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("user_input")
            .with_id("user_input")
            .with_tool(Arc::new(AskUserQuestionTool)),
    )
}

fn serialize_tool_value(value: impl Serialize) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(|error| ToolError::Execution {
        tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
        message: format!("failed to serialize clarifying-question data: {error}"),
    })
}

fn parse_arguments(arguments: Value) -> Result<AskUserQuestionArgs, ToolError> {
    serde_json::from_value(arguments).map_err(|error| ToolError::InvalidArguments {
        tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
        message: error.to_string(),
    })
}

/// Normalize and validate free-form or structured clarifying-question user input.
///
/// Durable hosts should use this helper before persisting or replaying an answer so their
/// behavior matches direct SDK execution.
///
/// # Errors
///
/// Returns [`ToolError::InvalidArguments`] when the payload is malformed, empty, or too long.
pub fn normalize_clarifying_question_answers(
    user_input: Value,
) -> Result<ClarifyingQuestionAnswers, ToolError> {
    let answers = match user_input {
        Value::String(response) => ClarifyingQuestionAnswers {
            answers: BTreeMap::new(),
            response: Some(response),
        },
        value => parse_answers(value)?,
    };
    validate_answer_payload(&answers)?;
    Ok(answers)
}

/// Build the canonical successful result for a durable clarifying-question answer.
///
/// # Errors
///
/// Returns [`ToolError::InvalidArguments`] when the questions or answer payload are malformed or
/// structured answer keys do not match the original questions.
pub fn resolve_clarifying_question_answers(
    questions: Value,
    user_input: Value,
) -> Result<AskUserQuestionResult, ToolError> {
    let questions: Vec<ClarifyingQuestion> =
        serde_json::from_value(questions).map_err(|error| ToolError::InvalidArguments {
            tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
            message: format!("invalid clarifying questions payload: {error}"),
        })?;
    validate_questions(&questions)?;
    let answers = normalize_clarifying_question_answers(user_input)?;
    validate_answers(&answers, &questions)?;
    Ok(AskUserQuestionResult {
        questions,
        answers: answers.answers,
        response: answers.response,
    })
}

fn parse_answers(value: Value) -> Result<ClarifyingQuestionAnswers, ToolError> {
    serde_json::from_value(value).map_err(|error| ToolError::InvalidArguments {
        tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
        message: format!("invalid clarifying answer payload: {error}"),
    })
}

fn validate_questions(questions: &[ClarifyingQuestion]) -> Result<(), ToolError> {
    if questions.is_empty() || questions.len() > MAX_QUESTIONS {
        return invalid_arguments(format!(
            "questions must contain between 1 and {MAX_QUESTIONS} items"
        ));
    }
    let mut question_texts = BTreeSet::new();
    for question in questions {
        let question_text = question.question.trim();
        if question_text.is_empty() {
            return invalid_arguments("question text must not be empty");
        }
        if !question_texts.insert(question_text) {
            return invalid_arguments("question text must be unique within one request");
        }
        let header_len = question.header.chars().count();
        if header_len == 0 || header_len > MAX_HEADER_CHARS {
            return invalid_arguments(format!(
                "question header must contain between 1 and {MAX_HEADER_CHARS} characters"
            ));
        }
        if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&question.options.len()) {
            return invalid_arguments(format!(
                "each question must contain between {MIN_OPTIONS} and {MAX_OPTIONS} options"
            ));
        }
        let mut option_labels = BTreeSet::new();
        for option in &question.options {
            let option_label = option.label.trim();
            if option_label.is_empty() || option.description.trim().is_empty() {
                return invalid_arguments("option labels and descriptions must not be empty");
            }
            if !option_labels.insert(option_label) {
                return invalid_arguments("option labels must be unique within one question");
            }
        }
    }
    Ok(())
}

fn validate_answers(
    answers: &ClarifyingQuestionAnswers,
    questions: &[ClarifyingQuestion],
) -> Result<(), ToolError> {
    validate_answer_payload(answers)?;
    if let Some(unknown) = answers.answers.keys().find(|question| {
        !questions
            .iter()
            .any(|item| item.question.as_str() == question.as_str())
    }) {
        return invalid_arguments(format!(
            "answer key does not match an asked question: {unknown}"
        ));
    }
    Ok(())
}

fn validate_answer_payload(answers: &ClarifyingQuestionAnswers) -> Result<(), ToolError> {
    let response_is_empty = answers
        .response
        .as_deref()
        .is_none_or(|response| response.trim().is_empty());
    if answers.answers.is_empty() && response_is_empty {
        return invalid_arguments("provide at least one answer or a free-form response");
    }
    if answers
        .response
        .as_ref()
        .is_some_and(|response| response.chars().count() > MAX_RESPONSE_CHARS)
    {
        return invalid_arguments(format!(
            "free-form response must not exceed {MAX_RESPONSE_CHARS} characters"
        ));
    }
    if answers
        .answers
        .iter()
        .any(|(question, answer)| question.trim().is_empty() || answer.trim().is_empty())
    {
        return invalid_arguments("answer keys and values must not be empty");
    }
    Ok(())
}

fn invalid_arguments<T>(message: impl Into<String>) -> Result<T, ToolError> {
    Err(ToolError::InvalidArguments {
        tool: ASK_USER_QUESTION_TOOL_NAME.to_string(),
        message: message.into(),
    })
}
