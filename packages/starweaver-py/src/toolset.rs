//! Python wrappers for Starweaver toolsets.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use pyo3::{exceptions::PyValueError, prelude::*};
use starweaver_context::AgentContext;
use starweaver_model::ToolDefinition;
use starweaver_tools::{
    ApprovalRequiredToolset, CombinedToolset, DeferredToolset, DynToolset, FilteredToolset,
    McpToolset, McpToolsetConfig, MetadataToolset, PrefixedToolset, RenamedToolset, StaticToolset,
    Tool, ToolContext, ToolError, ToolInstruction, ToolProxyToolset, ToolResult, ToolSearchToolset,
    ToolUserInputPreprocessResult, Toolset, ToolsetLifecycleError, ToolsetLifecyclePolicy,
    ToolsetLifecycleReport, ToolsetLifecycleState, ToolsetPreparation,
};

use crate::{
    context::PyToolsetContext,
    conversion::{py_to_json, serialize_to_py},
    tool::{PyPythonTool, py_tool_list_to_dyn_tools},
};

/// Python wrapper around a Starweaver toolset.
#[pyclass(name = "Toolset", skip_from_py_object)]
#[derive(Clone)]
pub struct PyToolset {
    inner: DynToolset,
}

impl PyToolset {
    pub(crate) fn new(inner: DynToolset) -> Self {
        Self { inner }
    }

    pub(crate) fn dyn_toolset(&self) -> DynToolset {
        self.inner.clone()
    }
}

/// Python wrapper around Rust toolset lifecycle policy.
#[pyclass(name = "ToolsetLifecyclePolicy", skip_from_py_object)]
#[derive(Clone)]
pub struct PyToolsetLifecyclePolicy {
    initialization_timeout_ms: Option<u64>,
    read_timeout_ms: Option<u64>,
    exit_timeout_ms: Option<u64>,
    enter_before_prepare: bool,
    exit_after_run: bool,
    fail_on_unavailable: bool,
}

impl PyToolsetLifecyclePolicy {
    const fn default_python_dynamic() -> Self {
        Self {
            initialization_timeout_ms: None,
            read_timeout_ms: None,
            exit_timeout_ms: None,
            enter_before_prepare: true,
            exit_after_run: true,
            fail_on_unavailable: false,
        }
    }

    fn policy(&self) -> ToolsetLifecyclePolicy {
        let mut policy = ToolsetLifecyclePolicy::default()
            .with_enter_before_prepare(self.enter_before_prepare)
            .with_exit_after_run(self.exit_after_run)
            .with_fail_on_unavailable(self.fail_on_unavailable);
        if let Some(timeout_ms) = self.initialization_timeout_ms {
            policy = policy.with_initialization_timeout_ms(timeout_ms);
        }
        if let Some(timeout_ms) = self.read_timeout_ms {
            policy = policy.with_read_timeout_ms(timeout_ms);
        }
        if let Some(timeout_ms) = self.exit_timeout_ms {
            policy = policy.with_exit_timeout_ms(timeout_ms);
        }
        policy
    }
}

struct PythonDynamicToolset {
    name: String,
    id: Option<String>,
    max_retries: Option<usize>,
    timeout_ms: Option<u64>,
    lifecycle_policy: ToolsetLifecyclePolicy,
    prepare_callback: Py<PyAny>,
    refresh_callback: Py<PyAny>,
    enter_callback: Py<PyAny>,
    exit_callback: Py<PyAny>,
    event_loop: Py<PyAny>,
    prepared_runs: Mutex<BTreeSet<String>>,
}

unsafe impl Send for PythonDynamicToolset {}
unsafe impl Sync for PythonDynamicToolset {}

impl PythonDynamicToolset {
    async fn call_callback(
        callback: &Py<PyAny>,
        event_loop: &Py<PyAny>,
        context: &AgentContext,
    ) -> PyResult<Py<PyAny>> {
        enum CallbackValue {
            Immediate(Py<PyAny>),
            Future(Py<PyAny>),
        }

        let context = PyToolsetContext::from_agent_context(context.clone()).await;
        let call_result = Python::attach(|py| -> PyResult<CallbackValue> {
            let context = Py::new(py, context)?;
            let value = callback.call1(py, (context,))?;
            let inspect = py.import("inspect")?;
            let is_awaitable = inspect
                .call_method1("isawaitable", (value.bind(py),))?
                .extract::<bool>()?;
            if !is_awaitable {
                return Ok(CallbackValue::Immediate(value));
            }
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (value, event_loop.clone_ref(py)),
            )?;
            Ok(CallbackValue::Future(future.unbind()))
        })?;

        let future = match call_result {
            CallbackValue::Immediate(value) => return Ok(value),
            CallbackValue::Future(future) => future,
        };
        let guard_future = Python::attach(|py| future.clone_ref(py));
        let mut cancel_guard = PythonCallbackCancelGuard::new(guard_future);

        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tick.tick().await;
            let poll = Python::attach(|py| -> PyResult<Option<Py<PyAny>>> {
                let done = future.call_method0(py, "done")?.extract::<bool>(py)?;
                if done {
                    Ok(Some(future.call_method0(py, "result")?))
                } else {
                    Ok(None)
                }
            });
            match poll {
                Ok(Some(value)) => break Ok(value),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
        };
        cancel_guard.complete();
        result
    }

    fn lifecycle_error(&self, operation: &str, error: PyErr) -> ToolsetLifecycleError {
        ToolsetLifecycleError::failed(
            self.name.clone(),
            format!("python toolset {operation} failed: {error}"),
        )
    }

    fn run_key(context: &AgentContext) -> String {
        context
            .run_id
            .as_ref()
            .map_or_else(
                || context.conversation_id.as_str(),
                |run_id| run_id.as_str(),
            )
            .to_string()
    }
}

struct PythonCallbackCancelGuard {
    future: Py<PyAny>,
    completed: bool,
}

impl PythonCallbackCancelGuard {
    fn new(future: Py<PyAny>) -> Self {
        Self {
            future,
            completed: false,
        }
    }

    const fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for PythonCallbackCancelGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        Python::attach(|py| {
            let _ = self.future.call_method0(py, "cancel");
        });
    }
}

#[async_trait]
impl Toolset for PythonDynamicToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<starweaver_tools::DynTool> {
        Vec::new()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        Vec::new()
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        self.lifecycle_policy
    }

    async fn enter_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        Self::call_callback(&self.enter_callback, &self.event_loop, context)
            .await
            .map_err(|error| self.lifecycle_error("enter", error))?;
        Ok(ToolsetLifecycleReport::new(
            self.name.clone(),
            self.id.clone(),
            ToolsetLifecycleState::Initialized,
            0,
            0,
        ))
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        let run_key = Self::run_key(context);
        let already_prepared = self
            .prepared_runs
            .lock()
            .map(|runs| runs.contains(&run_key))
            .unwrap_or(false);
        let (callback, state, operation) = if already_prepared {
            (
                &self.refresh_callback,
                ToolsetLifecycleState::Refreshed,
                "refresh",
            )
        } else {
            (
                &self.prepare_callback,
                ToolsetLifecycleState::Initialized,
                "prepare",
            )
        };
        let value = Self::call_callback(callback, &self.event_loop, context)
            .await
            .map_err(|error| self.lifecycle_error(operation, error))?;
        let prepared = Python::attach(|py| -> PyResult<DynToolset> {
            let toolset = value.bind(py).extract::<PyRef<'_, PyToolset>>()?;
            Ok(toolset.dyn_toolset())
        })
        .map_err(|error| self.lifecycle_error(operation, error))?;
        let nested = prepared
            .prepare_with_context(context)
            .await
            .map_err(|error| {
                self.lifecycle_error(operation, PyValueError::new_err(error.to_string()))
            })?;
        if !already_prepared && let Ok(mut runs) = self.prepared_runs.lock() {
            runs.insert(run_key);
        }
        let report = ToolsetLifecycleReport::new(
            self.name.clone(),
            self.id.clone(),
            state,
            nested.tools.len(),
            nested.instructions.len(),
        );
        Ok(ToolsetPreparation {
            tools: nested.tools,
            instructions: nested.instructions,
            report,
        })
    }

    async fn exit_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        Self::call_callback(&self.exit_callback, &self.event_loop, context)
            .await
            .map_err(|error| self.lifecycle_error("exit", error))?;
        Ok(ToolsetLifecycleReport::new(
            self.name.clone(),
            self.id.clone(),
            ToolsetLifecycleState::Closed,
            0,
            0,
        ))
    }
}

struct PythonPreparedToolset {
    name: String,
    id: Option<String>,
    inner: DynToolset,
    callback: Py<PyAny>,
    event_loop: Py<PyAny>,
}

unsafe impl Send for PythonPreparedToolset {}
unsafe impl Sync for PythonPreparedToolset {}

impl PythonPreparedToolset {
    fn new(inner: DynToolset, callback: Py<PyAny>, event_loop: Py<PyAny>) -> Self {
        let name = format!("{}_prepared", inner.name());
        let id = inner.id().map(|id| format!("{id}.prepared"));
        Self {
            name,
            id,
            inner,
            callback,
            event_loop,
        }
    }

    fn lifecycle_error(&self, operation: &str, error: PyErr) -> ToolsetLifecycleError {
        ToolsetLifecycleError::failed(
            self.name.clone(),
            format!("python prepared toolset {operation} failed: {error}"),
        )
    }

    async fn call_prepare_callback(
        &self,
        context: &AgentContext,
        definitions: &[ToolDefinition],
    ) -> PyResult<Option<Vec<ToolDefinition>>> {
        enum CallbackValue {
            Immediate(Py<PyAny>),
            Future(Py<PyAny>),
        }

        let context = PyToolsetContext::from_agent_context(context.clone()).await;
        let call_result = Python::attach(|py| -> PyResult<CallbackValue> {
            let context = Py::new(py, context)?;
            let definitions = serialize_to_py(py, &definitions.to_vec())?;
            let value = self.callback.call1(py, (context, definitions))?;
            let inspect = py.import("inspect")?;
            let is_awaitable = inspect
                .call_method1("isawaitable", (value.bind(py),))?
                .extract::<bool>()?;
            if !is_awaitable {
                return Ok(CallbackValue::Immediate(value));
            }
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (value, self.event_loop.clone_ref(py)),
            )?;
            Ok(CallbackValue::Future(future.unbind()))
        })?;

        let future = match call_result {
            CallbackValue::Immediate(value) => {
                return Python::attach(|py| definitions_from_callback_value(py, value));
            }
            CallbackValue::Future(future) => future,
        };
        let guard_future = Python::attach(|py| future.clone_ref(py));
        let mut cancel_guard = PythonCallbackCancelGuard::new(guard_future);

        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tick.tick().await;
            let poll = Python::attach(|py| -> PyResult<Option<Py<PyAny>>> {
                let done = future.call_method0(py, "done")?.extract::<bool>(py)?;
                if done {
                    Ok(Some(future.call_method0(py, "result")?))
                } else {
                    Ok(None)
                }
            });
            match poll {
                Ok(Some(value)) => {
                    break Python::attach(|py| definitions_from_callback_value(py, value));
                }
                Ok(None) => {}
                Err(error) => break Err(error),
            }
        };
        cancel_guard.complete();
        result
    }

    fn wrap_prepared_tools(
        &self,
        context: &AgentContext,
        tools: Vec<starweaver_tools::DynTool>,
        definitions: Option<Vec<ToolDefinition>>,
    ) -> Result<Vec<starweaver_tools::DynTool>, ToolsetLifecycleError> {
        let mut prepared = Vec::new();
        let mut original_names = BTreeSet::new();
        for tool in tools {
            if !tool.is_available(context) {
                continue;
            }
            let Some(definition) = tool.prepare_definition(context, tool.definition()) else {
                continue;
            };
            original_names.insert(definition.name.clone());
            prepared.push((tool, definition));
        }

        let Some(definitions) = definitions else {
            return Ok(prepared
                .into_iter()
                .map(|(tool, definition)| {
                    Arc::new(PreparedDefinitionTool {
                        inner: tool,
                        definition,
                    }) as starweaver_tools::DynTool
                })
                .collect());
        };

        let mut by_name = BTreeMap::new();
        for definition in definitions {
            if !original_names.contains(&definition.name) {
                return Err(ToolsetLifecycleError::failed(
                    self.name.clone(),
                    format!(
                        "prepared callback returned unknown tool definition {:?}",
                        definition.name
                    ),
                ));
            }
            if by_name
                .insert(definition.name.clone(), definition)
                .is_some()
            {
                return Err(ToolsetLifecycleError::failed(
                    self.name.clone(),
                    "prepared callback returned duplicate tool definitions",
                ));
            }
        }

        Ok(prepared
            .into_iter()
            .filter_map(|(tool, _)| {
                by_name.remove(tool.name()).map(|definition| {
                    Arc::new(PreparedDefinitionTool {
                        inner: tool,
                        definition,
                    }) as starweaver_tools::DynTool
                })
            })
            .collect())
    }

    fn wrapper_report(
        &self,
        mut report: ToolsetLifecycleReport,
        tool_count: usize,
        instruction_count: usize,
    ) -> ToolsetLifecycleReport {
        report.name.clone_from(&self.name);
        report.id.clone_from(&self.id);
        report.tool_count = tool_count;
        report.instruction_count = instruction_count;
        report
    }
}

#[async_trait]
impl Toolset for PythonPreparedToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<starweaver_tools::DynTool> {
        self.inner.get_tools()
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.inner.timeout_ms()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner.get_instructions()
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        self.inner.lifecycle_policy()
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        let preparation = self.inner.prepare_with_context(context).await?;
        let definitions = self
            .call_prepare_callback(
                context,
                &prepared_definitions_for_context(context, &preparation.tools),
            )
            .await
            .map_err(|error| self.lifecycle_error("callback", error))?;
        let tools = self.wrap_prepared_tools(context, preparation.tools, definitions)?;
        let instructions = preparation.instructions;
        let report = self.wrapper_report(preparation.report, tools.len(), instructions.len());
        Ok(ToolsetPreparation {
            tools,
            instructions,
            report,
        })
    }

    async fn enter_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        let report = self.inner.enter_with_context(context).await?;
        Ok(self.wrapper_report(
            report,
            self.get_tools().len(),
            self.get_instructions().len(),
        ))
    }

    async fn exit_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        let report = self.inner.exit_with_context(context).await?;
        Ok(self.wrapper_report(report, 0, 0))
    }
}

struct PreparedDefinitionTool {
    inner: starweaver_tools::DynTool,
    definition: ToolDefinition,
}

#[async_trait]
impl Tool for PreparedDefinitionTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> Option<&str> {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    fn metadata(&self) -> starweaver_core::Metadata {
        self.inner.metadata()
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.inner.timeout_ms()
    }

    fn return_schema(&self) -> Option<serde_json::Value> {
        self.inner.return_schema()
    }

    fn strict_schema(&self) -> Option<bool> {
        self.inner.strict_schema()
    }

    fn sequential(&self) -> Option<bool> {
        self.inner.sequential()
    }

    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn is_available(&self, context: &AgentContext) -> bool {
        self.inner.is_available(context)
    }

    fn prepare_definition(
        &self,
        _context: &AgentContext,
        _definition: ToolDefinition,
    ) -> Option<ToolDefinition> {
        Some(self.definition.clone())
    }

    async fn call(
        &self,
        context: ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        self.inner.call(context, arguments).await
    }

    async fn preprocess_user_input(
        &self,
        context: ToolContext,
        user_input: serde_json::Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        self.inner.preprocess_user_input(context, user_input).await
    }
}

fn prepared_definitions_for_context(
    context: &AgentContext,
    tools: &[starweaver_tools::DynTool],
) -> Vec<ToolDefinition> {
    tools
        .iter()
        .filter(|tool| tool.is_available(context))
        .filter_map(|tool| tool.prepare_definition(context, tool.definition()))
        .collect()
}

fn definitions_from_callback_value(
    py: Python<'_>,
    value: Py<PyAny>,
) -> PyResult<Option<Vec<ToolDefinition>>> {
    if value.bind(py).is_none() {
        return Ok(None);
    }
    let value = py_to_json(py, value.bind(py))?;
    serde_json::from_value::<Vec<ToolDefinition>>(value)
        .map(Some)
        .map_err(|error| PyValueError::new_err(format!("invalid prepared definitions: {error}")))
}

#[pymethods]
impl PyToolsetLifecyclePolicy {
    #[new]
    #[pyo3(signature = (initialization_timeout_ms=None, read_timeout_ms=None, exit_timeout_ms=None, enter_before_prepare=true, exit_after_run=true, fail_on_unavailable=false))]
    const fn new_py(
        initialization_timeout_ms: Option<u64>,
        read_timeout_ms: Option<u64>,
        exit_timeout_ms: Option<u64>,
        enter_before_prepare: bool,
        exit_after_run: bool,
        fail_on_unavailable: bool,
    ) -> Self {
        Self {
            initialization_timeout_ms,
            read_timeout_ms,
            exit_timeout_ms,
            enter_before_prepare,
            exit_after_run,
            fail_on_unavailable,
        }
    }

    #[getter]
    const fn initialization_timeout_ms(&self) -> Option<u64> {
        self.initialization_timeout_ms
    }

    #[getter]
    const fn read_timeout_ms(&self) -> Option<u64> {
        self.read_timeout_ms
    }

    #[getter]
    const fn exit_timeout_ms(&self) -> Option<u64> {
        self.exit_timeout_ms
    }

    #[getter]
    const fn enter_before_prepare(&self) -> bool {
        self.enter_before_prepare
    }

    #[getter]
    const fn exit_after_run(&self) -> bool {
        self.exit_after_run
    }

    #[getter]
    const fn fail_on_unavailable(&self) -> bool {
        self.fail_on_unavailable
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.policy())
    }
}

#[pymethods]
impl PyToolset {
    #[new]
    #[pyo3(signature = (name, tools=None, instructions=None, id=None, max_retries=None, timeout_ms=None))]
    fn new_py(
        py: Python<'_>,
        name: String,
        tools: Option<Vec<Py<PyPythonTool>>>,
        instructions: Option<Vec<String>>,
        id: Option<String>,
        max_retries: Option<usize>,
        timeout_ms: Option<u64>,
    ) -> PyResult<Self> {
        let mut toolset = StaticToolset::new(name.clone());
        if let Some(id) = id {
            toolset = toolset.with_id(id);
        }
        for tool in py_tool_list_to_dyn_tools(py, tools)? {
            toolset = toolset.with_tool(tool);
        }
        for instruction in instructions.unwrap_or_default() {
            toolset = toolset.with_instruction(ToolInstruction::new(name.clone(), instruction));
        }
        if let Some(max_retries) = max_retries {
            toolset = toolset.with_max_retries(max_retries);
        }
        if let Some(timeout_ms) = timeout_ms {
            toolset = toolset.with_timeout_ms(timeout_ms);
        }
        Ok(Self::new(Arc::new(toolset)))
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name().to_string()
    }

    #[getter]
    fn id(&self) -> Option<String> {
        self.inner.id().map(ToOwned::to_owned)
    }

    fn tool_definitions(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let definitions = self
            .inner
            .get_tools()
            .into_iter()
            .map(|tool| tool.definition())
            .collect::<Vec<_>>();
        serialize_to_py(py, &definitions)
    }

    fn instructions(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.get_instructions())
    }
}

/// Build a direct dynamic tool-search toolset.
#[pyfunction]
pub fn tool_search_toolset(
    toolsets: Vec<Py<PyToolset>>,
    max_results: Option<usize>,
    py: Python<'_>,
) -> PyResult<PyToolset> {
    let inner_toolsets = py_toolsets_to_dyn_toolsets(py, Some(toolsets))?;
    let mut toolset = ToolSearchToolset::new(inner_toolsets);
    if let Some(max_results) = max_results {
        toolset = toolset.with_max_results(max_results);
    }
    Ok(PyToolset::new(Arc::new(toolset)))
}

/// Build a fixed two-tool proxy over wrapped toolsets.
#[pyfunction]
pub fn tool_proxy_toolset(
    toolsets: Vec<Py<PyToolset>>,
    prefix: Option<String>,
    max_results: Option<usize>,
    namespace_descriptions: Option<std::collections::BTreeMap<String, String>>,
    py: Python<'_>,
) -> PyResult<PyToolset> {
    let inner_toolsets = py_toolsets_to_dyn_toolsets(py, Some(toolsets))?;
    let mut toolset = ToolProxyToolset::new(inner_toolsets);
    if let Some(prefix) = prefix {
        toolset = toolset
            .try_with_name_prefix(prefix)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
    }
    if let Some(max_results) = max_results {
        toolset = toolset.with_max_results(max_results);
    }
    if let Some(namespace_descriptions) = namespace_descriptions {
        toolset = toolset.with_namespace_descriptions(namespace_descriptions);
    }
    Ok(PyToolset::new(Arc::new(toolset)))
}

/// Build a combined toolset over several native toolsets.
#[pyfunction]
#[pyo3(signature = (name, toolsets, id=None, max_retries=None, timeout_ms=None))]
pub fn combined_toolset(
    py: Python<'_>,
    name: String,
    toolsets: Vec<Py<PyToolset>>,
    id: Option<String>,
    max_retries: Option<usize>,
    timeout_ms: Option<u64>,
) -> PyResult<PyToolset> {
    let inner_toolsets = py_toolsets_to_dyn_toolsets(py, Some(toolsets))?;
    let mut toolset = CombinedToolset::new(name, inner_toolsets);
    if let Some(id) = id {
        toolset = toolset.with_id(id);
    }
    if let Some(max_retries) = max_retries {
        toolset = toolset.with_max_retries(max_retries);
    }
    if let Some(timeout_ms) = timeout_ms {
        toolset = toolset.with_timeout_ms(timeout_ms);
    }
    Ok(PyToolset::new(Arc::new(toolset)))
}

/// Build a deferred-call MCP toolset from typed MCP configuration.
#[pyfunction]
pub fn mcp_toolset(config: &Bound<'_, PyAny>, py: Python<'_>) -> PyResult<PyToolset> {
    let config = serde_json::from_value::<McpToolsetConfig>(py_to_json(py, config)?)
        .map_err(|error| PyValueError::new_err(format!("invalid MCP toolset config: {error}")))?;
    if config.id.trim().is_empty() {
        return Err(PyValueError::new_err("MCP toolset id must not be empty"));
    }
    Ok(PyToolset::new(Arc::new(McpToolset::new(config))))
}

/// Build a prepared-callback toolset wrapper.
#[pyfunction]
pub fn prepared_toolset(
    toolset: Py<PyToolset>,
    callback: Py<PyAny>,
    event_loop: Py<PyAny>,
    py: Python<'_>,
) -> PyToolset {
    PyToolset::new(Arc::new(PythonPreparedToolset::new(
        toolset.borrow(py).dyn_toolset(),
        callback,
        event_loop,
    )))
}

/// Build a dynamic Python-backed toolset.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (name, prepare_callback, refresh_callback, enter_callback, exit_callback, event_loop, id=None, max_retries=None, timeout_ms=None, lifecycle_policy=None))]
pub fn dynamic_toolset(
    py: Python<'_>,
    name: String,
    prepare_callback: Py<PyAny>,
    refresh_callback: Py<PyAny>,
    enter_callback: Py<PyAny>,
    exit_callback: Py<PyAny>,
    event_loop: Py<PyAny>,
    id: Option<String>,
    max_retries: Option<usize>,
    timeout_ms: Option<u64>,
    lifecycle_policy: Option<Py<PyToolsetLifecyclePolicy>>,
) -> PyResult<PyToolset> {
    if name.trim().is_empty() {
        return Err(PyValueError::new_err("toolset name must not be empty"));
    }
    let lifecycle_policy = lifecycle_policy
        .as_ref()
        .map(|policy| policy.borrow(py).policy())
        .unwrap_or_else(|| PyToolsetLifecyclePolicy::default_python_dynamic().policy());
    Ok(PyToolset::new(Arc::new(PythonDynamicToolset {
        name,
        id,
        max_retries,
        timeout_ms,
        lifecycle_policy,
        prepare_callback,
        refresh_callback,
        enter_callback,
        exit_callback,
        event_loop,
        prepared_runs: Mutex::new(BTreeSet::new()),
    })))
}

/// Build a prefixed toolset wrapper.
#[pyfunction]
pub fn prefixed_toolset(toolset: Py<PyToolset>, prefix: String, py: Python<'_>) -> PyToolset {
    PyToolset::new(Arc::new(PrefixedToolset::new(
        prefix,
        toolset.borrow(py).dyn_toolset(),
    )))
}

/// Build a statically filtered toolset wrapper.
#[pyfunction]
#[pyo3(signature = (toolset, include=None, exclude=None))]
pub fn filtered_toolset(
    toolset: Py<PyToolset>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    py: Python<'_>,
) -> PyResult<PyToolset> {
    let inner = toolset.borrow(py).dyn_toolset();
    let wrapper = match (include, exclude) {
        (Some(include), None) => FilteredToolset::include_names(inner, include),
        (None, Some(exclude)) => FilteredToolset::exclude_names(inner, exclude),
        (None, None) => {
            return Err(PyValueError::new_err(
                "filtered_toolset requires include or exclude",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(PyValueError::new_err(
                "filtered_toolset accepts include or exclude, not both",
            ));
        }
    };
    Ok(PyToolset::new(Arc::new(wrapper)))
}

/// Build a renamed toolset wrapper.
#[pyfunction]
pub fn renamed_toolset(
    toolset: Py<PyToolset>,
    mappings: BTreeMap<String, String>,
    py: Python<'_>,
) -> PyToolset {
    PyToolset::new(Arc::new(RenamedToolset::new(
        toolset.borrow(py).dyn_toolset(),
        mappings,
    )))
}

/// Build a toolset wrapper that merges metadata into every exposed tool.
#[pyfunction]
pub fn metadata_toolset(
    toolset: Py<PyToolset>,
    metadata: &Bound<'_, PyAny>,
    py: Python<'_>,
) -> PyResult<PyToolset> {
    let metadata = match py_to_json(py, metadata)? {
        serde_json::Value::Object(metadata) => metadata,
        _ => {
            return Err(PyValueError::new_err(
                "metadata_toolset metadata must be an object",
            ));
        }
    };
    Ok(PyToolset::new(Arc::new(MetadataToolset::new(
        toolset.borrow(py).dyn_toolset(),
        metadata,
    ))))
}

/// Build an approval-required toolset wrapper.
#[pyfunction]
#[pyo3(signature = (toolset, names, reason=None))]
pub fn approval_required_toolset(
    toolset: Py<PyToolset>,
    names: Vec<String>,
    reason: Option<String>,
    py: Python<'_>,
) -> PyToolset {
    let mut wrapper = ApprovalRequiredToolset::new(toolset.borrow(py).dyn_toolset(), names);
    if let Some(reason) = reason {
        wrapper = wrapper.with_reason(reason);
    }
    PyToolset::new(Arc::new(wrapper))
}

/// Build a deferred-call toolset wrapper.
#[pyfunction]
#[pyo3(signature = (toolset, names, reason=None))]
pub fn deferred_toolset(
    toolset: Py<PyToolset>,
    names: Vec<String>,
    reason: Option<String>,
    py: Python<'_>,
) -> PyToolset {
    let mut wrapper = DeferredToolset::new(toolset.borrow(py).dyn_toolset(), names);
    if let Some(reason) = reason {
        wrapper = wrapper.with_reason(reason);
    }
    PyToolset::new(Arc::new(wrapper))
}

/// Build the first-party filesystem toolset backed by an attached environment.
#[pyfunction]
pub fn filesystem_toolset() -> PyToolset {
    PyToolset::new(starweaver_agent::filesystem_tools())
}

/// Build the first-party foreground shell toolset backed by an attached environment.
#[pyfunction]
pub fn shell_toolset() -> PyToolset {
    PyToolset::new(starweaver_agent::shell_tools())
}

/// Build the first-party filesystem and shell toolsets.
#[pyfunction]
pub fn environment_toolsets() -> Vec<PyToolset> {
    starweaver_agent::environment_toolsets()
        .into_iter()
        .map(PyToolset::new)
        .collect()
}

pub(crate) fn py_toolsets_to_dyn_toolsets(
    py: Python<'_>,
    toolsets: Option<Vec<Py<PyToolset>>>,
) -> PyResult<Vec<DynToolset>> {
    let mut result = Vec::new();
    for toolset in toolsets.unwrap_or_default() {
        result.push(toolset.borrow(py).dyn_toolset());
    }
    Ok(result)
}
