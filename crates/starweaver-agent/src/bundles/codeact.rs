use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use rquickjs::{CatchResultExt, Context, Function, Object, Runtime};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_environment::FileListOptions;
use starweaver_tools::{
    CodeActEligibility, DynTool, DynToolset, NestedToolInvoker, StaticToolset,
    TOOL_METADATA_DEPENDENCIES_KEY, TOOL_METADATA_NESTED_CALL_LIMIT_KEY,
    TOOL_METADATA_NESTED_INVOKER_KEY, TOOL_METADATA_NESTED_RESULT_MAX_BYTES_KEY, Tool, ToolContext,
    ToolDependencyRequirements, ToolError, ToolResult, Toolset, ToolsetLifecycleError,
    ToolsetLifecyclePolicy, ToolsetLifecycleReport, ToolsetLifecycleState, ToolsetPreparation,
};

use super::EnvironmentHandle;

/// Canonical model-facing constrained JavaScript tool name.
pub const RUN_CODE_TOOL_NAME: &str = "run_code";
/// Stable `CodeAct` toolset identifier.
pub const CODEACT_TOOLSET_ID: &str = "starweaver.codeact";
/// Stable file-backed recipe toolset identifier.
pub const RECIPE_TOOLSET_ID: &str = "starweaver.recipes";

const MAX_RECIPE_LIST_ENTRIES: usize = 512;
const MAX_RECIPES: usize = 128;
const MAX_RECIPE_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_RECIPE_SCHEMA_BYTES: usize = 256 * 1024;
const MAX_RECIPE_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_RECIPE_TARGETS: usize = 64;
const RECIPE_PREPARATION_TIMEOUT_MS: u64 = 30_000;

/// Resource limits applied to one constrained source evaluation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodeExecutionLimits {
    /// Maximum UTF-8 source bytes.
    pub max_source_bytes: usize,
    /// Maximum serialized input bytes.
    pub max_input_bytes: usize,
    /// Maximum serialized output bytes.
    pub max_output_bytes: usize,
    /// `QuickJS` heap limit in bytes.
    pub memory_bytes: usize,
    /// `QuickJS` stack limit in bytes.
    pub stack_bytes: usize,
    /// Total evaluation deadline in milliseconds.
    pub timeout_ms: u64,
    /// Maximum child calls admitted by the runtime broker.
    pub max_tool_calls: usize,
}

impl Default for CodeExecutionLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 64 * 1024,
            max_input_bytes: 64 * 1024,
            max_output_bytes: 256 * 1024,
            memory_bytes: 32 * 1024 * 1024,
            stack_bytes: 512 * 1024,
            timeout_ms: 30_000,
            max_tool_calls: 64,
        }
    }
}

/// One immutable constrained-code execution request.
#[derive(Clone, Debug)]
pub struct CodeExecutionRequest {
    /// JavaScript source defining `function main(input)`.
    pub source: Arc<str>,
    /// JSON input passed to `main`.
    pub input: Value,
    /// Source digest used for evidence and recipe provenance.
    pub source_digest: String,
    /// Effective resource limits.
    pub limits: CodeExecutionLimits,
    /// Cooperative cancellation inherited from the owning tool call.
    pub cancellation_token: starweaver_core::CancellationToken,
}

/// Successful constrained-code execution result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodeExecutionResult {
    /// JSON-compatible return value from `main`.
    pub value: Value,
    /// Source digest evaluated by the executor.
    pub source_digest: String,
    /// Number of elapsed wall-clock milliseconds.
    pub duration_ms: u64,
}

/// Constrained executor failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CodeExecutionError {
    /// Source or input exceeds a configured bound.
    #[error("code execution input exceeds limit: {0}")]
    Limit(String),
    /// Source uses a prohibited language or host feature.
    #[error("prohibited JavaScript feature: {0}")]
    Prohibited(String),
    /// JavaScript parsing or evaluation failed.
    #[error("JavaScript execution failed: {0}")]
    JavaScript(String),
    /// The owning run cancelled execution.
    #[error("code execution cancelled")]
    Cancelled,
    /// The execution deadline elapsed.
    #[error("code execution timed out")]
    Timeout,
    /// The nested bridge reported a terminal runtime or policy failure.
    #[error("terminal nested tool bridge failure: {0}")]
    ToolBridge(String),
    /// Blocking executor worker failed.
    #[error("code executor worker failed: {0}")]
    Worker(String),
}

/// Replaceable constrained-code executor contract.
#[async_trait]
pub trait CodeExecutor: Send + Sync {
    /// Execute one request using only the supplied exact-name tool bridge.
    async fn execute(
        &self,
        request: CodeExecutionRequest,
        tools: NestedToolInvoker,
    ) -> Result<CodeExecutionResult, CodeExecutionError>;
}

/// Shared constrained-code executor reference.
pub type DynCodeExecutor = Arc<dyn CodeExecutor>;

/// QuickJS-backed constrained synchronous JavaScript executor.
#[derive(Clone, Debug, Default)]
pub struct QuickJsCodeExecutor;

#[async_trait]
impl CodeExecutor for QuickJsCodeExecutor {
    #[allow(clippy::too_many_lines)]
    async fn execute(
        &self,
        request: CodeExecutionRequest,
        tools: NestedToolInvoker,
    ) -> Result<CodeExecutionResult, CodeExecutionError> {
        validate_request(&request)?;
        let started_at = Instant::now();
        let deadline = started_at + Duration::from_millis(request.limits.timeout_ms);
        let timed_out = Arc::new(AtomicBool::new(false));
        let timeout_flag = timed_out.clone();
        let cancellation = request.cancellation_token.clone();
        let tools = tools.with_execution_control(
            tokio::time::Instant::from_std(deadline),
            request.cancellation_token.clone(),
        );
        let runtime_handle = tokio::runtime::Handle::current();
        let source_digest = request.source_digest.clone();
        let output = tokio::task::spawn_blocking(move || {
            let terminal_bridge_error = Arc::new(Mutex::new(None::<String>));
            let bridge_enabled = Arc::new(AtomicBool::new(false));
            let pre_main_bridge_attempted = Arc::new(AtomicBool::new(false));
            let runtime = Runtime::new()
                .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
            runtime.set_memory_limit(request.limits.memory_bytes);
            runtime.set_max_stack_size(request.limits.stack_bytes);
            runtime.set_interrupt_handler(Some(Box::new(move || {
                if cancellation.is_cancelled() {
                    return true;
                }
                if Instant::now() >= deadline {
                    timeout_flag.store(true, Ordering::Release);
                    return true;
                }
                false
            })));
            let context = Context::full(&runtime)
                .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
            let output = context.with(|ctx| {
                ctx.eval::<(), _>(SANDBOX_PRELUDE)
                    .catch(&ctx)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                let bridge = tools.clone();
                let handle = runtime_handle.clone();
                let terminal_error_for_call = terminal_bridge_error.clone();
                let bridge_enabled_for_call = bridge_enabled.clone();
                let pre_main_attempt_for_call = pre_main_bridge_attempted.clone();
                let max_bridge_result_bytes = request.limits.max_output_bytes;
                let host_call = Function::new(
                    ctx.clone(),
                    move |name: String, arguments_json: String| -> rquickjs::Result<String> {
                        let envelope = if bridge_enabled_for_call.load(Ordering::Acquire) {
                            let retained_terminal = terminal_error_for_call
                                .lock()
                                .ok()
                                .and_then(|retained| retained.clone());
                            retained_terminal.map_or_else(
                                || {
                                    match serde_json::from_str::<Value>(&arguments_json) {
                            Ok(arguments) => {
                                match handle.block_on(bridge.invoke(name, arguments)) {
                                    Ok(result) => match serde_json::to_vec(&result.content) {
                                        Ok(bytes) if bytes.len() <= max_bridge_result_bytes => {
                                            serde_json::json!({"ok": true, "value": result.content})
                                        }
                                        Ok(_) => {
                                            let message = "nested tool result exceeds the configured output byte limit".to_string();
                                            if let Ok(mut retained) = terminal_error_for_call.lock() {
                                                retained.get_or_insert_with(|| message.clone());
                                            }
                                            serde_json::json!({
                                                "ok": false,
                                                "error": message,
                                                "terminal": true,
                                            })
                                        }
                                        Err(error) => {
                                            let message = format!("nested tool result is not JSON: {error}");
                                            if let Ok(mut retained) = terminal_error_for_call.lock() {
                                                retained.get_or_insert_with(|| message.clone());
                                            }
                                            serde_json::json!({
                                                "ok": false,
                                                "error": message,
                                                "terminal": true,
                                            })
                                        }
                                    }
                                    Err(error) => {
                                        let terminal = !matches!(
                                            error,
                                            starweaver_tools::NestedToolError::ToolFailed { .. }
                                        );
                                        if terminal
                                            && let Ok(mut retained) = terminal_error_for_call.lock()
                                        {
                                            retained.get_or_insert_with(|| error.to_string());
                                        }
                                        serde_json::json!({
                                            "ok": false,
                                            "error": error.to_string(),
                                            "terminal": terminal,
                                        })
                                    }
                                }
                            }
                                        Err(error) => serde_json::json!({
                                            "ok": false,
                                            "error": format!("tool arguments are not JSON: {error}"),
                                        }),
                                    }
                                },
                                |message| {
                                    serde_json::json!({
                                        "ok": false,
                                        "error": message,
                                        "terminal": true,
                                    })
                                },
                            )
                        } else {
                            pre_main_attempt_for_call.store(true, Ordering::Release);
                            serde_json::json!({
                                "ok": false,
                                "error": "tools.call is unavailable before main execution",
                            })
                        };
                        serde_json::to_string(&envelope).map_err(|error| {
                            rquickjs::Error::new_from_js_message(
                                "nested tool result",
                                "JSON",
                                error.to_string(),
                            )
                        })
                    },
                )
                .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                let bridge_factory = ctx
                    .eval::<Function, _>(TOOL_BRIDGE_FACTORY)
                    .catch(&ctx)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                let tool_bridge = bridge_factory
                    .call::<_, Object>((host_call,))
                    .catch(&ctx)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                let input = serde_json::to_string(&request.input)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                let program_source = format!(
                    "(tools) => {{\n\"use strict\";\n{}\nif (typeof main !== \"function\") throw new TypeError(\"source must define function main(input)\");\nreturn main;\n}}",
                    request.source,
                );
                let program_factory = ctx
                    .eval::<Function, _>(program_source)
                    .catch(&ctx)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                let main = program_factory
                    .call::<_, Function>((tool_bridge,))
                    .catch(&ctx)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                if pre_main_bridge_attempted.load(Ordering::Acquire) {
                    return Err(CodeExecutionError::JavaScript(
                        "tools.call is unavailable before main execution".to_string(),
                    ));
                }
                bridge_enabled.store(true, Ordering::Release);
                let invocation = ctx
                    .eval::<Function, _>(MAIN_INVOCATION)
                    .catch(&ctx)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
                invocation
                    .call::<_, String>((main, input))
                    .catch(&ctx)
                    .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))
            });
            let terminal_error = terminal_bridge_error
                .lock()
                .map_err(|error| CodeExecutionError::Worker(error.to_string()))?
                .clone();
            if let Some(error) = terminal_error {
                return Err(CodeExecutionError::ToolBridge(error));
            }
            output
        })
        .await
        .map_err(|error| CodeExecutionError::Worker(error.to_string()))?;

        if request.cancellation_token.is_cancelled() {
            return Err(CodeExecutionError::Cancelled);
        }
        if timed_out.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(CodeExecutionError::Timeout);
        }
        let output = output?;
        if output.len() > request.limits.max_output_bytes {
            return Err(CodeExecutionError::Limit("output bytes".to_string()));
        }
        let value = serde_json::from_str(&output).map_err(|error| {
            CodeExecutionError::JavaScript(format!("result is not JSON: {error}"))
        })?;
        Ok(CodeExecutionResult {
            value,
            source_digest,
            duration_ms: u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

const SANDBOX_PRELUDE: &str = r#"
"use strict";
(() => {
  const define = Object.defineProperty;
  const getPrototypeOf = Object.getPrototypeOf;
  for (const prototype of [
    Function.prototype,
    getPrototypeOf(function* () {}),
    getPrototypeOf(async function () {}),
    getPrototypeOf(async function* () {})
  ]) {
    define(prototype, "constructor", {
      value: undefined,
      writable: false,
      configurable: false
    });
    Object.freeze(prototype);
  }
  for (const name of [
    "eval", "Function", "Promise", "WebAssembly", "Date", "console", "fetch",
    "XMLHttpRequest", "Worker", "SharedArrayBuffer", "Atomics"
  ]) {
    define(globalThis, name, { value: undefined, writable: false, configurable: false });
  }
  define(Math, "random", { value: undefined, writable: false, configurable: false });
})();
"#;

const TOOL_BRIDGE_FACTORY: &str = r#"
(hostCall) => Object.freeze({
  call(name, args) {
    if (typeof name !== "string" || name.length === 0) {
      throw new TypeError("tools.call requires a non-empty canonical tool name");
    }
    const serialized = JSON.stringify(args);
    if (serialized === undefined) {
      throw new TypeError("tools.call arguments must be JSON-serializable");
    }
    const response = JSON.parse(hostCall(name, serialized));
    if (!response.ok) throw new Error(response.error);
    return response.value;
  }
})
"#;

const MAIN_INVOCATION: &str = r#"
(main, __starweaver_input) => {
  "use strict";
  const result = main(JSON.parse(__starweaver_input));
  if (result && typeof result.then === "function") throw new TypeError("async results are not allowed");
  const serialized = JSON.stringify(result);
  if (serialized === undefined) throw new TypeError("main result must be JSON-serializable");
  return serialized;
}
"#;

fn validate_request(request: &CodeExecutionRequest) -> Result<(), CodeExecutionError> {
    if request.limits.max_source_bytes == 0
        || request.limits.max_input_bytes == 0
        || request.limits.max_output_bytes == 0
        || request.limits.memory_bytes == 0
        || request.limits.stack_bytes == 0
        || request.limits.timeout_ms == 0
    {
        return Err(CodeExecutionError::Limit(
            "configured byte, memory, stack, and timeout limits must be positive".to_string(),
        ));
    }
    if request.source.len() > request.limits.max_source_bytes {
        return Err(CodeExecutionError::Limit("source bytes".to_string()));
    }
    let input_bytes = serde_json::to_vec(&request.input)
        .map_err(|error| CodeExecutionError::JavaScript(error.to_string()))?;
    if input_bytes.len() > request.limits.max_input_bytes {
        return Err(CodeExecutionError::Limit("input bytes".to_string()));
    }
    let prohibited = [
        ("async", "async functions"),
        ("await", "await"),
        ("import", "modules/import"),
        ("export", "modules/export"),
        ("eval", "dynamic eval"),
        ("Function", "dynamic Function"),
        ("Promise", "promises"),
        ("WebAssembly", "WebAssembly"),
        ("Date", "host clock"),
        ("Math.random", "randomness"),
        ("console", "host console"),
        ("globalThis", "global object access"),
    ];
    if let Some((_, feature)) = prohibited
        .iter()
        .find(|(token, _)| contains_identifier_or_path(&request.source, token))
    {
        return Err(CodeExecutionError::Prohibited((*feature).to_string()));
    }
    Ok(())
}

fn contains_identifier_or_path(source: &str, token: &str) -> bool {
    source.match_indices(token).any(|(index, _)| {
        let before = source[..index].chars().next_back();
        let after = source[index + token.len()..].chars().next();
        !before.is_some_and(is_identifier_continue) && !after.is_some_and(is_identifier_continue)
    })
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character == '$' || character.is_alphanumeric()
}

/// Arguments accepted by [`RUN_CODE_TOOL_NAME`].
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct RunCodeArgs {
    /// Synchronous JavaScript defining `function main(input)`.
    pub source: String,
    /// JSON value supplied to `main`.
    #[serde(default)]
    pub input: Value,
}

/// SDK `CodeAct` bundle configuration.
#[derive(Clone)]
pub struct CodeActConfig {
    executor: DynCodeExecutor,
    limits: CodeExecutionLimits,
}

impl std::fmt::Debug for CodeActConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodeActConfig")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Default for CodeActConfig {
    fn default() -> Self {
        Self {
            executor: Arc::new(QuickJsCodeExecutor),
            limits: CodeExecutionLimits::default(),
        }
    }
}

impl CodeActConfig {
    /// Build configuration with a replaceable executor.
    #[must_use]
    pub fn new(executor: DynCodeExecutor) -> Self {
        Self {
            executor,
            limits: CodeExecutionLimits::default(),
        }
    }

    /// Override host-bounded execution limits.
    #[must_use]
    pub const fn with_limits(mut self, limits: CodeExecutionLimits) -> Self {
        self.limits = limits;
        self
    }
}

/// Install standard exact grants needed by `CodeAct`-safe first-party context tools.
///
/// Products call this for fresh and restored contexts. Environment and Computer Use grants remain
/// attached by their owning adapters.
pub fn attach_codeact_standard_grants(context: &mut AgentContext) {
    super::task::attach_task_tool_grants(context);
}

/// Create the SDK `CodeAct` toolset. Generic SDK applications opt in explicitly.
#[must_use]
pub fn codeact_tools(config: CodeActConfig) -> DynToolset {
    Arc::new(
        StaticToolset::new("codeact")
            .with_id(CODEACT_TOOLSET_ID)
            .with_tool(Arc::new(RunCodeTool { config }) as DynTool),
    )
}

/// Create the context-prepared file-backed recipe toolset using the same executor policy.
#[must_use]
pub fn recipe_tools(config: CodeActConfig) -> DynToolset {
    Arc::new(RecipeToolset::new(config.executor).with_limits(config.limits))
}

struct RunCodeTool {
    config: CodeActConfig,
}

#[async_trait]
impl Tool for RunCodeTool {
    fn name(&self) -> &str {
        RUN_CODE_TOOL_NAME
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Run bounded synchronous JavaScript defining function main(input). Compose available exact-name tools with tools.call(\"tool_name\", {...}). JSON only; no async, modules, filesystem, network, process, clock, randomness, console, eval, Function, or WebAssembly. Interrupted source is never automatically rerun.",
        )
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = serde_json::to_value(schemars::schema_for!(RunCodeArgs))
            .unwrap_or_else(|_| serde_json::json!({"type": "object"}));
        if let Some(object) = schema.as_object_mut() {
            object.remove("$schema");
        }
        schema
    }

    fn metadata(&self) -> Metadata {
        orchestration_metadata(
            "codeact",
            self.config.limits.max_tool_calls,
            self.config.limits.max_output_bytes,
        )
    }

    fn max_retries(&self) -> Option<usize> {
        Some(0)
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(self.config.limits.timeout_ms)
    }

    fn sequential(&self) -> Option<bool> {
        Some(true)
    }

    fn codeact_eligibility(&self, _context: &AgentContext) -> CodeActEligibility {
        CodeActEligibility::Deny
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        let arguments = serde_json::from_value::<RunCodeArgs>(arguments).map_err(|error| {
            ToolError::InvalidArguments {
                tool: RUN_CODE_TOOL_NAME.to_string(),
                message: error.to_string(),
            }
        })?;
        let Some(invoker) = context.dependency::<NestedToolInvoker>() else {
            return Err(ToolError::UserError {
                tool: RUN_CODE_TOOL_NAME.to_string(),
                message: "runtime did not admit a nested tool invoker".to_string(),
            });
        };
        let source_digest = source_digest(arguments.source.as_bytes());
        let result = self
            .config
            .executor
            .execute(
                CodeExecutionRequest {
                    source: Arc::from(arguments.source),
                    input: arguments.input,
                    source_digest: source_digest.clone(),
                    limits: self.config.limits.clone(),
                    cancellation_token: context.cancellation_token(),
                },
                (*invoker).clone(),
            )
            .await
            .map_err(|error| ToolError::UserError {
                tool: RUN_CODE_TOOL_NAME.to_string(),
                message: error.to_string(),
            })?;
        let mut tool_result = ToolResult::new(serde_json::json!({
            "status": "completed",
            "value": result.value,
            "source_digest": result.source_digest,
            "duration_ms": result.duration_ms,
        }));
        tool_result
            .metadata
            .insert("source_digest".to_string(), Value::String(source_digest));
        Ok(tool_result)
    }
}

fn orchestration_metadata(
    bundle: &str,
    max_tool_calls: usize,
    max_result_bytes: usize,
) -> Metadata {
    let mut metadata = Metadata::from_iter([
        ("bundle".to_string(), Value::String(bundle.to_string())),
        (
            TOOL_METADATA_NESTED_INVOKER_KEY.to_string(),
            Value::Bool(true),
        ),
        (
            TOOL_METADATA_NESTED_CALL_LIMIT_KEY.to_string(),
            serde_json::json!(max_tool_calls),
        ),
        (
            TOOL_METADATA_NESTED_RESULT_MAX_BYTES_KEY.to_string(),
            serde_json::json!(max_result_bytes),
        ),
    ]);
    metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::strict(Vec::<String>::new(), Vec::<String>::new(), false)
            .to_metadata_value(),
    );
    metadata
}

fn source_digest(source: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(source))
}

/// Versioned file-backed recipe manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeManifest {
    /// Manifest format version. The initial version is `1`.
    pub version: u32,
    /// Canonical exposed recipe tool name.
    pub name: String,
    /// Optional model-facing description.
    #[serde(default)]
    pub description: Option<String>,
    /// Source file relative to the recipe directory.
    #[serde(default = "default_recipe_source")]
    pub source: String,
    /// Optional expected SHA-256 digest (`sha256:<hex>`).
    #[serde(default)]
    pub source_digest: Option<String>,
    /// Complete maximum canonical target allowlist.
    #[serde(default)]
    pub tools: BTreeSet<String>,
    /// Optional JSON input schema file relative to the recipe directory.
    #[serde(default)]
    pub input_schema: Option<String>,
}

fn default_recipe_source() -> String {
    "main.js".to_string()
}

/// Errors returned while pinning file-backed recipes.
#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    /// Active environment attachment is unavailable.
    #[error("recipe toolset requires an active EnvironmentHandle")]
    MissingEnvironment,
    /// Provider operation failed.
    #[error("recipe provider error: {0}")]
    Provider(String),
    /// Manifest is malformed or unsupported.
    #[error("invalid recipe manifest {path}: {message}")]
    Manifest {
        /// Provider-visible manifest path.
        path: String,
        /// Safe validation message.
        message: String,
    },
}

/// Context-prepared file-backed recipe toolset.
pub struct RecipeToolset {
    root: String,
    executor: DynCodeExecutor,
    limits: CodeExecutionLimits,
}

impl std::fmt::Debug for RecipeToolset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecipeToolset")
            .field("root", &self.root)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl RecipeToolset {
    /// Build a recipe toolset rooted at `.starweaver/recipes`.
    #[must_use]
    pub fn new(executor: DynCodeExecutor) -> Self {
        Self {
            root: ".starweaver/recipes".to_string(),
            executor,
            limits: CodeExecutionLimits::default(),
        }
    }

    /// Override the provider-visible configured recipe root.
    #[must_use]
    pub fn with_root(mut self, root: impl Into<String>) -> Self {
        self.root = root.into();
        self
    }

    /// Override limits applied to every pinned recipe.
    #[must_use]
    pub const fn with_limits(mut self, limits: CodeExecutionLimits) -> Self {
        self.limits = limits;
        self
    }

    #[allow(clippy::too_many_lines)]
    async fn pin_recipes(&self, context: &AgentContext) -> Result<Vec<DynTool>, RecipeError> {
        let environment = context
            .dependencies
            .get::<EnvironmentHandle>()
            .ok_or(RecipeError::MissingEnvironment)?;
        let provider = environment.provider();
        let listing = match provider
            .list_with_options(
                &self.root,
                FileListOptions {
                    ignore_patterns: Vec::new(),
                    max_entries: MAX_RECIPE_LIST_ENTRIES + 1,
                },
            )
            .await
        {
            Ok(listing) => listing,
            Err(starweaver_environment::EnvironmentError::NotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(RecipeError::Provider(error.to_string())),
        };
        if listing.truncated || listing.total_entries > MAX_RECIPE_LIST_ENTRIES {
            return Err(RecipeError::Manifest {
                path: self.root.clone(),
                message: format!(
                    "recipe root exceeds the {MAX_RECIPE_LIST_ENTRIES}-entry inventory limit"
                ),
            });
        }
        let recipe_dirs = collect_recipe_directories(&self.root, listing.entries)?;
        if recipe_dirs.len() > MAX_RECIPES {
            return Err(RecipeError::Manifest {
                path: self.root.clone(),
                message: format!("recipe root exceeds the {MAX_RECIPES}-recipe limit"),
            });
        }
        let mut tools = Vec::new();
        let mut names = BTreeSet::new();
        for recipe_dir in recipe_dirs {
            let manifest_path = format!("{recipe_dir}/recipe.toml");
            let manifest_bytes = match provider
                .read_bytes(&manifest_path, 0, Some(MAX_RECIPE_MANIFEST_BYTES + 1))
                .await
            {
                Ok(bytes) => bytes,
                Err(starweaver_environment::EnvironmentError::NotFound(_)) => continue,
                Err(error) => return Err(RecipeError::Provider(error.to_string())),
            };
            let manifest_text = decode_bounded_recipe_text(
                &manifest_path,
                manifest_bytes,
                MAX_RECIPE_MANIFEST_BYTES,
                "manifest",
            )?;
            let manifest = toml::from_str::<RecipeManifest>(&manifest_text).map_err(|error| {
                RecipeError::Manifest {
                    path: manifest_path.clone(),
                    message: error.to_string(),
                }
            })?;
            validate_manifest(&manifest, &manifest_path)?;
            if !names.insert(manifest.name.clone()) {
                return Err(RecipeError::Manifest {
                    path: manifest_path,
                    message: format!("duplicate recipe name {:?}", manifest.name),
                });
            }
            let source_path = join_recipe_path(&recipe_dir, &manifest.source, &manifest_path)?;
            let source = read_bounded_recipe_text(
                &provider,
                &source_path,
                self.limits.max_source_bytes,
                "source",
            )
            .await?;
            let digest = source_digest(source.as_bytes());
            if manifest
                .source_digest
                .as_ref()
                .is_some_and(|expected| expected != &digest)
            {
                return Err(RecipeError::Manifest {
                    path: source_path,
                    message: "declared source_digest does not match exact source bytes".to_string(),
                });
            }
            let (input_schema, input_validator, schema_snapshot) = load_recipe_input_schema(
                &provider,
                &recipe_dir,
                &manifest_path,
                manifest.input_schema.as_deref(),
            )
            .await?;
            let confirmed_manifest = read_bounded_recipe_text(
                &provider,
                &manifest_path,
                MAX_RECIPE_MANIFEST_BYTES,
                "manifest",
            )
            .await?;
            if confirmed_manifest != manifest_text {
                return Err(RecipeError::Manifest {
                    path: manifest_path.clone(),
                    message: "manifest changed while the recipe bundle was being pinned"
                        .to_string(),
                });
            }
            let confirmed_source = read_bounded_recipe_text(
                &provider,
                &source_path,
                self.limits.max_source_bytes,
                "source",
            )
            .await?;
            if confirmed_source != source {
                return Err(RecipeError::Manifest {
                    path: source_path,
                    message: "source changed while the recipe bundle was being pinned".to_string(),
                });
            }
            if let Some((schema_path, schema_text)) = schema_snapshot {
                let confirmed_schema = read_bounded_recipe_text(
                    &provider,
                    &schema_path,
                    MAX_RECIPE_SCHEMA_BYTES,
                    "input schema",
                )
                .await?;
                if confirmed_schema != schema_text {
                    return Err(RecipeError::Manifest {
                        path: schema_path,
                        message: "input schema changed while the recipe bundle was being pinned"
                            .to_string(),
                    });
                }
            }
            let manifest_digest = source_digest(manifest_text.as_bytes());
            let input_schema_digest = source_digest(
                &serde_json::to_vec(&input_schema)
                    .map_err(|error| RecipeError::Provider(error.to_string()))?,
            );
            tools.push(Arc::new(RecipeTool {
                manifest,
                manifest_digest,
                source: Arc::from(source),
                source_digest: digest,
                input_schema,
                input_schema_digest,
                input_validator,
                executor: self.executor.clone(),
                limits: self.limits.clone(),
            }) as DynTool);
        }
        Ok(tools)
    }
}

#[async_trait]
impl Toolset for RecipeToolset {
    fn name(&self) -> &'static str {
        "recipes"
    }

    fn id(&self) -> Option<&str> {
        Some(RECIPE_TOOLSET_ID)
    }

    fn get_tools(&self) -> Vec<DynTool> {
        Vec::new()
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        ToolsetLifecyclePolicy::default().with_read_timeout_ms(RECIPE_PREPARATION_TIMEOUT_MS)
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        let tools = self
            .pin_recipes(context)
            .await
            .map_err(|error| ToolsetLifecycleError::failed(self.name(), error.to_string()))?;
        let report = ToolsetLifecycleReport {
            name: self.name().to_string(),
            id: self.id().map(ToOwned::to_owned),
            state: ToolsetLifecycleState::Refreshed,
            tool_count: tools.len(),
            instruction_count: 0,
            message: None,
            metadata: Metadata::new(),
        };
        Ok(ToolsetPreparation {
            tools,
            instructions: Vec::new(),
            report,
        })
    }
}

struct RecipeTool {
    manifest: RecipeManifest,
    manifest_digest: String,
    source: Arc<str>,
    source_digest: String,
    input_schema: Value,
    input_schema_digest: String,
    input_validator: Arc<jsonschema::Validator>,
    executor: DynCodeExecutor,
    limits: CodeExecutionLimits,
}

#[async_trait]
impl Tool for RecipeTool {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn description(&self) -> Option<&str> {
        self.manifest.description.as_deref()
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn metadata(&self) -> Metadata {
        let mut metadata = orchestration_metadata(
            "recipes",
            self.limits.max_tool_calls,
            self.limits.max_output_bytes,
        );
        metadata.insert(
            "recipe_manifest_digest".to_string(),
            Value::String(self.manifest_digest.clone()),
        );
        metadata.insert(
            "recipe_source_digest".to_string(),
            Value::String(self.source_digest.clone()),
        );
        metadata.insert(
            "recipe_input_schema_digest".to_string(),
            Value::String(self.input_schema_digest.clone()),
        );
        metadata
    }

    fn max_retries(&self) -> Option<usize> {
        Some(0)
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(self.limits.timeout_ms)
    }

    fn sequential(&self) -> Option<bool> {
        Some(true)
    }

    fn codeact_eligibility(&self, _context: &AgentContext) -> CodeActEligibility {
        CodeActEligibility::Deny
    }

    async fn call(&self, context: ToolContext, input: Value) -> Result<ToolResult, ToolError> {
        if let Err(error) = self.input_validator.validate(&input) {
            return Err(ToolError::InvalidArguments {
                tool: self.manifest.name.clone(),
                message: format!("recipe input does not match its pinned schema: {error}"),
            });
        }
        let Some(invoker) = context.dependency::<NestedToolInvoker>() else {
            return Err(ToolError::UserError {
                tool: self.manifest.name.clone(),
                message: "runtime did not admit a nested tool invoker".to_string(),
            });
        };
        if !self
            .manifest
            .tools
            .iter()
            .all(|name| invoker.allowed_tools().contains(name))
        {
            return Err(ToolError::UserError {
                tool: self.manifest.name.clone(),
                message: "recipe declares a target outside the pinned CodeAct projection"
                    .to_string(),
            });
        }
        let result = self
            .executor
            .execute(
                CodeExecutionRequest {
                    source: self.source.clone(),
                    input,
                    source_digest: self.source_digest.clone(),
                    limits: self.limits.clone(),
                    cancellation_token: context.cancellation_token(),
                },
                invoker.restricted_to(&self.manifest.tools),
            )
            .await
            .map_err(|error| ToolError::UserError {
                tool: self.manifest.name.clone(),
                message: error.to_string(),
            })?;
        Ok(ToolResult::new(serde_json::json!({
            "status": "completed",
            "value": result.value,
            "recipe": self.manifest.name,
            "source_digest": result.source_digest,
            "duration_ms": result.duration_ms,
        })))
    }
}

async fn load_recipe_input_schema(
    provider: &starweaver_environment::DynEnvironmentProvider,
    recipe_dir: &str,
    manifest_path: &str,
    schema_path: Option<&str>,
) -> Result<(Value, Arc<jsonschema::Validator>, Option<(String, String)>), RecipeError> {
    let Some(schema_path) = schema_path else {
        let schema = serde_json::json!({"type": "object"});
        let validator =
            jsonschema::validator_for(&schema).map_err(|error| RecipeError::Manifest {
                path: manifest_path.to_string(),
                message: format!("invalid default JSON input schema: {error}"),
            })?;
        return Ok((schema, Arc::new(validator), None));
    };
    let path = join_recipe_path(recipe_dir, schema_path, manifest_path)?;
    let text =
        read_bounded_recipe_text(provider, &path, MAX_RECIPE_SCHEMA_BYTES, "input schema").await?;
    let schema = serde_json::from_str(&text).map_err(|error| RecipeError::Manifest {
        path: path.clone(),
        message: format!("invalid JSON input schema: {error}"),
    })?;
    let validator = jsonschema::validator_for(&schema).map_err(|error| RecipeError::Manifest {
        path: path.clone(),
        message: format!("invalid JSON input schema: {error}"),
    })?;
    Ok((schema, Arc::new(validator), Some((path, text))))
}

async fn read_bounded_recipe_text(
    provider: &starweaver_environment::DynEnvironmentProvider,
    path: &str,
    max_bytes: usize,
    resource: &str,
) -> Result<String, RecipeError> {
    let bytes = provider
        .read_bytes(path, 0, Some(max_bytes.saturating_add(1)))
        .await
        .map_err(|error| RecipeError::Provider(error.to_string()))?;
    decode_bounded_recipe_text(path, bytes, max_bytes, resource)
}

fn decode_bounded_recipe_text(
    path: &str,
    bytes: Vec<u8>,
    max_bytes: usize,
    resource: &str,
) -> Result<String, RecipeError> {
    if bytes.len() > max_bytes {
        return Err(RecipeError::Manifest {
            path: path.to_string(),
            message: format!("{resource} exceeds the configured {max_bytes}-byte limit"),
        });
    }
    String::from_utf8(bytes).map_err(|error| RecipeError::Manifest {
        path: path.to_string(),
        message: format!("{resource} is not valid UTF-8: {error}"),
    })
}

fn collect_recipe_directories(
    root: &str,
    entries: Vec<String>,
) -> Result<BTreeSet<String>, RecipeError> {
    let mut recipe_dirs = BTreeSet::new();
    for entry in entries {
        let Some(entry) = normalize_recipe_entry(root, &entry)? else {
            continue;
        };
        if let Some(recipe_dir) = entry.strip_suffix("/recipe.toml") {
            recipe_dirs.insert(recipe_dir.to_string());
        } else {
            // Providers may return shallow directory entries rather than recursive files.
            recipe_dirs.insert(entry);
        }
    }
    Ok(recipe_dirs)
}

fn normalize_recipe_entry(root: &str, entry: &str) -> Result<Option<String>, RecipeError> {
    let root = if root == "/" {
        root
    } else {
        root.trim_end_matches('/')
    };
    if root.is_empty() {
        return Err(RecipeError::Manifest {
            path: root.to_string(),
            message: "recipe root must not be empty".to_string(),
        });
    }
    if entry == root {
        return Ok(None);
    }
    let prefix = if root == "/" {
        "/".to_string()
    } else {
        format!("{root}/")
    };
    let (normalized, relative) = if let Some(relative) = entry.strip_prefix(&prefix) {
        (entry.to_string(), relative)
    } else {
        if entry.starts_with('/') || entry.contains('\\') {
            return Err(RecipeError::Manifest {
                path: entry.to_string(),
                message: "provider returned an entry outside the configured recipe root"
                    .to_string(),
            });
        }
        (format!("{prefix}{entry}"), entry)
    };
    if relative.is_empty()
        || relative
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(RecipeError::Manifest {
            path: entry.to_string(),
            message: "provider returned a non-contained recipe entry".to_string(),
        });
    }
    Ok(Some(normalized))
}

fn validate_manifest(manifest: &RecipeManifest, path: &str) -> Result<(), RecipeError> {
    if manifest.version != 1 {
        return Err(RecipeError::Manifest {
            path: path.to_string(),
            message: format!("unsupported version {}; expected 1", manifest.version),
        });
    }
    if manifest.name.trim().is_empty()
        || !manifest.name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        return Err(RecipeError::Manifest {
            path: path.to_string(),
            message: "name must use the non-empty ASCII [A-Za-z0-9_-] alphabet".to_string(),
        });
    }
    if manifest.name == RUN_CODE_TOOL_NAME || manifest.tools.contains(&manifest.name) {
        return Err(RecipeError::Manifest {
            path: path.to_string(),
            message: "recipe recursion is not allowed".to_string(),
        });
    }
    if manifest
        .description
        .as_ref()
        .is_some_and(|description| description.len() > MAX_RECIPE_DESCRIPTION_BYTES)
    {
        return Err(RecipeError::Manifest {
            path: path.to_string(),
            message: format!("description exceeds the {MAX_RECIPE_DESCRIPTION_BYTES}-byte limit"),
        });
    }
    if manifest.tools.len() > MAX_RECIPE_TARGETS {
        return Err(RecipeError::Manifest {
            path: path.to_string(),
            message: format!("tools exceeds the {MAX_RECIPE_TARGETS}-target limit"),
        });
    }
    Ok(())
}

fn join_recipe_path(
    recipe_dir: &str,
    relative: &str,
    manifest_path: &str,
) -> Result<String, RecipeError> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative
            .split('/')
            .any(|segment| matches!(segment, "." | "..") || segment.is_empty())
        || relative.contains('\\')
    {
        return Err(RecipeError::Manifest {
            path: manifest_path.to_string(),
            message: format!("recipe resource path {relative:?} must be a contained relative path"),
        });
    }
    Ok(format!("{}/{}", recipe_dir.trim_end_matches('/'), relative))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use starweaver_tools::{NestedToolError, NestedToolResult};

    #[tokio::test]
    async fn quickjs_executor_composes_exact_name_tool_calls() {
        let (invoker, mut receiver) =
            NestedToolInvoker::channel(BTreeSet::from(["echo-tool".to_string()]), 1);
        let broker = tokio::spawn(async move {
            let request = receiver.recv().await.expect("one nested request");
            assert_eq!(request.tool_name, "echo-tool");
            assert_eq!(request.arguments, serde_json::json!({"value": 8}));
            request
                .response
                .send(Ok(NestedToolResult {
                    content: serde_json::json!({"value": 8}),
                    metadata: serde_json::Map::new(),
                }))
                .expect("executor should await nested result");
        });
        let source = Arc::<str>::from(
            "function main(input) { return tools.call(\"echo-tool\", { value: input.value + 1 }); }",
        );
        let result = QuickJsCodeExecutor
            .execute(
                CodeExecutionRequest {
                    source: source.clone(),
                    input: serde_json::json!({"value": 7}),
                    source_digest: source_digest(source.as_bytes()),
                    limits: CodeExecutionLimits::default(),
                    cancellation_token: starweaver_core::CancellationToken::default(),
                },
                invoker,
            )
            .await
            .expect("constrained code should complete");
        broker.await.expect("broker task should complete");
        assert_eq!(result.value, serde_json::json!({"value": 8}));
    }

    #[tokio::test]
    async fn quickjs_executor_cannot_catch_terminal_bridge_failures() {
        let (invoker, mut receiver) =
            NestedToolInvoker::channel(BTreeSet::from(["limited".to_string()]), 1);
        let broker = tokio::spawn(async move {
            let request = receiver.recv().await.expect("one nested request");
            request
                .response
                .send(Err(NestedToolError::CallLimit))
                .expect("executor should await nested result");
        });
        let source = Arc::<str>::from(
            "function main() { try { tools.call(\"limited\", {}); } catch (_) { return { bypassed: true }; } }",
        );
        let error = QuickJsCodeExecutor
            .execute(
                CodeExecutionRequest {
                    source: source.clone(),
                    input: Value::Null,
                    source_digest: source_digest(source.as_bytes()),
                    limits: CodeExecutionLimits::default(),
                    cancellation_token: starweaver_core::CancellationToken::default(),
                },
                invoker,
            )
            .await
            .expect_err("terminal bridge errors must override a caught JavaScript exception");
        broker.await.expect("broker task should complete");
        assert!(matches!(error, CodeExecutionError::ToolBridge(_)));
    }

    #[tokio::test]
    async fn quickjs_executor_rejects_caught_top_level_tool_calls_before_admission() {
        let (invoker, mut receiver) =
            NestedToolInvoker::channel(BTreeSet::from(["effect".to_string()]), 1);
        let source = Arc::<str>::from(
            "try { tools.call(\"effect\", {}); } catch (_) {} function main() { return { completed: true }; }",
        );
        let error = QuickJsCodeExecutor
            .execute(
                CodeExecutionRequest {
                    source: source.clone(),
                    input: Value::Null,
                    source_digest: source_digest(source.as_bytes()),
                    limits: CodeExecutionLimits::default(),
                    cancellation_token: starweaver_core::CancellationToken::default(),
                },
                invoker,
            )
            .await
            .expect_err("top-level tool attempts must fail before broker admission");

        assert!(matches!(error, CodeExecutionError::JavaScript(_)));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn quickjs_executor_rejects_dynamic_source_before_tool_admission() {
        let (invoker, mut receiver) = NestedToolInvoker::channel(BTreeSet::new(), 1);
        let source = Arc::<str>::from("function main() { return eval(\"1 + 1\"); }");
        let error = QuickJsCodeExecutor
            .execute(
                CodeExecutionRequest {
                    source: source.clone(),
                    input: Value::Null,
                    source_digest: source_digest(source.as_bytes()),
                    limits: CodeExecutionLimits::default(),
                    cancellation_token: starweaver_core::CancellationToken::default(),
                },
                invoker,
            )
            .await
            .expect_err("eval must be rejected");
        assert!(matches!(error, CodeExecutionError::Prohibited(_)));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn quickjs_executor_severs_intrinsic_dynamic_constructor_paths() {
        for source in [
            "function main() { return ({}).constructor.constructor(\"return 4\")(); }",
            "function main() { return ({} )[\"con\" + \"structor\"][\"con\" + \"structor\"](\"return 4\")(); }",
            "function main() { return (function* () {}).constructor(\"yield 4\")().next().value; }",
        ] {
            let source = Arc::<str>::from(source);
            let (invoker, _receiver) = NestedToolInvoker::channel(BTreeSet::new(), 1);
            let error = QuickJsCodeExecutor
                .execute(
                    CodeExecutionRequest {
                        source: source.clone(),
                        input: Value::Null,
                        source_digest: source_digest(source.as_bytes()),
                        limits: CodeExecutionLimits::default(),
                        cancellation_token: starweaver_core::CancellationToken::default(),
                    },
                    invoker,
                )
                .await
                .expect_err("intrinsic constructors must not compile dynamic source");
            assert!(matches!(error, CodeExecutionError::JavaScript(_)));
        }

        let source = Arc::<str>::from(
            "function main() { return { bridge: typeof __starweaver_call, tools: typeof tools }; }",
        );
        let (invoker, _receiver) = NestedToolInvoker::channel(BTreeSet::new(), 1);
        let result = QuickJsCodeExecutor
            .execute(
                CodeExecutionRequest {
                    source: source.clone(),
                    input: Value::Null,
                    source_digest: source_digest(source.as_bytes()),
                    limits: CodeExecutionLimits::default(),
                    cancellation_token: starweaver_core::CancellationToken::default(),
                },
                invoker,
            )
            .await
            .expect("only the lexical public tools bridge should be visible");
        assert_eq!(
            result.value,
            serde_json::json!({"bridge": "undefined", "tools": "object"})
        );
    }

    #[tokio::test]
    async fn quickjs_executor_deadline_bounds_an_unresponsive_broker() {
        let (invoker, _receiver) =
            NestedToolInvoker::channel(BTreeSet::from(["never".to_string()]), 1);
        let source = Arc::<str>::from("function main() { return tools.call(\"never\", {}); }");
        let limits = CodeExecutionLimits {
            timeout_ms: 20,
            ..CodeExecutionLimits::default()
        };
        let started_at = Instant::now();
        let error = QuickJsCodeExecutor
            .execute(
                CodeExecutionRequest {
                    source: source.clone(),
                    input: Value::Null,
                    source_digest: source_digest(source.as_bytes()),
                    limits,
                    cancellation_token: starweaver_core::CancellationToken::default(),
                },
                invoker,
            )
            .await
            .expect_err("the absolute deadline must interrupt a broker wait");
        assert!(matches!(error, CodeExecutionError::Timeout));
        assert!(started_at.elapsed() < Duration::from_millis(500));
    }
}
