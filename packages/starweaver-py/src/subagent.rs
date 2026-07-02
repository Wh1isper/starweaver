//! Python wrappers for SDK subagent configuration.

use std::sync::Arc;

use pyo3::{exceptions::PyTypeError, prelude::*};
use starweaver_agent::{
    SubagentCapabilityInheritancePolicy, SubagentConfig, SubagentDelegationMode,
    SubagentToolInheritancePolicy,
};

use crate::agent::PyAgent;

/// Python wrapper around a Starweaver subagent configuration.
#[pyclass(name = "Subagent", skip_from_py_object)]
#[derive(Clone)]
pub struct PySubagent {
    inner: SubagentConfig,
}

impl PySubagent {
    pub(crate) fn config(&self) -> SubagentConfig {
        self.inner.clone()
    }
}

#[pymethods]
impl PySubagent {
    #[new]
    #[pyo3(signature = (name, agent, description=None, required_tools=None, optional_tools=None, denied_tools=None, auto_inherit=true, inherit_all_when_empty=false, allow_nested_delegation=false, inherit_hooks=false, inherit_capability_bundles=false, denied_capabilities=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        agent: &Bound<'_, PyAny>,
        description: Option<String>,
        required_tools: Option<Vec<String>>,
        optional_tools: Option<Vec<String>>,
        denied_tools: Option<Vec<String>>,
        auto_inherit: bool,
        inherit_all_when_empty: bool,
        allow_nested_delegation: bool,
        inherit_hooks: bool,
        inherit_capability_bundles: bool,
        denied_capabilities: Option<Vec<String>>,
    ) -> PyResult<Self> {
        let agent = agent
            .extract::<PyRef<'_, PyAgent>>()
            .map_err(|_| PyTypeError::new_err("subagent agent must be a starweaver.Agent"))?;
        let mut tool_policy = SubagentToolInheritancePolicy::new(
            required_tools.unwrap_or_default(),
            optional_tools.unwrap_or_default(),
        )
        .with_denied_tools(denied_tools.unwrap_or_default())
        .with_inherit_all_when_empty(inherit_all_when_empty)
        .with_nested_delegation(allow_nested_delegation);
        if !auto_inherit {
            tool_policy = tool_policy.without_auto_inherit();
        }
        let capability_policy = SubagentCapabilityInheritancePolicy::default()
            .with_hooks(inherit_hooks)
            .with_capability_bundles(inherit_capability_bundles)
            .with_denied_capabilities(denied_capabilities.unwrap_or_default());
        let mut config = SubagentConfig::new(name, Arc::new(agent.runtime_agent()))
            .with_tool_inheritance(tool_policy)
            .with_capability_inheritance(capability_policy);
        if let Some(description) = description {
            config = config.with_description(description);
        }
        Ok(Self { inner: config })
    }
}

pub(crate) fn parse_delegation_mode(value: Option<String>) -> PyResult<SubagentDelegationMode> {
    match value.as_deref().unwrap_or("blocking") {
        "blocking" => Ok(SubagentDelegationMode::Blocking),
        "async" => Ok(SubagentDelegationMode::Async),
        "blocking_and_async" => Ok(SubagentDelegationMode::BlockingAndAsync),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "unsupported subagent delegation mode: {other}"
        ))),
    }
}
