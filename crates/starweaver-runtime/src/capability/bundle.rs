//! Capability bundle contracts and static bundle implementation.

use std::sync::Arc;

use starweaver_model::{ModelRequestParameters, ModelSettings};
use starweaver_tools::{DynTool, DynToolset, ToolRegistry};

use starweaver_usage::UsageLimits;

use crate::{
    instructions::DynDynamicInstruction,
    output::{DynOutputFunction, OutputValidator},
};

use super::{AgentCapability, CapabilitySpec};

/// Composable agent extension that contributes hooks, tools, instructions, and settings.
pub trait CapabilityBundle: Send + Sync {
    /// Bundle name for diagnostics and registry surfaces.
    fn name(&self) -> &str;

    /// Stable capability spec for bundle-level reconstruction evidence.
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(self.name())
    }

    /// Capability hooks contributed by this bundle.
    fn hooks(&self) -> Vec<Arc<dyn AgentCapability>> {
        Vec::new()
    }

    /// Stream observer hooks contributed by this bundle.
    fn stream_observers(&self) -> Vec<Arc<dyn AgentCapability>> {
        Vec::new()
    }

    /// Static instructions contributed by this bundle.
    fn get_instructions(&self) -> Vec<String> {
        Vec::new()
    }

    /// Dynamic instructions contributed by this bundle.
    fn dynamic_instructions(&self) -> Vec<DynDynamicInstruction> {
        Vec::new()
    }

    /// Runtime tools and tool instructions contributed by this bundle.
    fn get_tools(&self) -> Option<ToolRegistry> {
        None
    }

    /// Model settings overlay contributed by this bundle.
    fn model_settings(&self) -> Option<ModelSettings> {
        None
    }

    /// Request parameter overlay contributed by this bundle.
    fn request_params(&self) -> Option<ModelRequestParameters> {
        None
    }

    /// Output functions contributed by this bundle.
    fn output_functions(&self) -> Vec<DynOutputFunction> {
        Vec::new()
    }

    /// Output validators contributed by this bundle.
    fn output_validators(&self) -> Vec<Arc<dyn OutputValidator>> {
        Vec::new()
    }

    /// Usage limits contributed by this bundle.
    fn usage_limits(&self) -> Option<UsageLimits> {
        None
    }
}

/// Static capability bundle for reusable runtime composition.
#[derive(Clone, Default)]
pub struct StaticCapabilityBundle {
    name: String,
    spec: Option<CapabilitySpec>,
    hooks: Vec<Arc<dyn AgentCapability>>,
    stream_observers: Vec<Arc<dyn AgentCapability>>,
    instructions: Vec<String>,
    dynamic_instructions: Vec<DynDynamicInstruction>,
    tools: ToolRegistry,
    model_settings: Option<ModelSettings>,
    request_params: Option<ModelRequestParameters>,
    output_functions: Vec<DynOutputFunction>,
    output_validators: Vec<Arc<dyn OutputValidator>>,
    usage_limits: Option<UsageLimits>,
}

impl StaticCapabilityBundle {
    /// Create an empty static capability bundle.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            spec: None,
            hooks: Vec::new(),
            stream_observers: Vec::new(),
            instructions: Vec::new(),
            dynamic_instructions: Vec::new(),
            tools: ToolRegistry::new(),
            model_settings: None,
            request_params: None,
            output_functions: Vec::new(),
            output_validators: Vec::new(),
            usage_limits: None,
        }
    }

    /// Set the bundle capability spec.
    #[must_use]
    pub fn with_spec(mut self, spec: CapabilitySpec) -> Self {
        self.spec = Some(spec);
        self
    }

    /// Add a capability hook.
    #[must_use]
    pub fn with_hook(mut self, hook: Arc<dyn AgentCapability>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Add a stream observer hook.
    #[must_use]
    pub fn with_stream_observer(mut self, observer: Arc<dyn AgentCapability>) -> Self {
        self.stream_observers.push(observer);
        self
    }

    /// Add a static instruction.
    #[must_use]
    pub fn with_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
    }

    /// Add a dynamic instruction.
    #[must_use]
    pub fn with_dynamic_instruction(mut self, instruction: DynDynamicInstruction) -> Self {
        self.dynamic_instructions.push(instruction);
        self
    }

    /// Add one tool.
    #[must_use]
    pub fn with_tool(mut self, tool: DynTool) -> Self {
        self.tools.insert(tool);
        self
    }

    /// Add one toolset.
    #[must_use]
    pub fn with_toolset(mut self, toolset: &DynToolset) -> Self {
        self.tools.insert_toolset(toolset);
        self
    }

    /// Set model settings overlay.
    #[must_use]
    pub fn with_model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Set request parameter overlay.
    #[must_use]
    pub fn with_request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = Some(params);
        self
    }

    /// Add an output function.
    #[must_use]
    pub fn with_output_function(mut self, function: DynOutputFunction) -> Self {
        self.output_functions.push(function);
        self
    }

    /// Add an output validator.
    #[must_use]
    pub fn with_output_validator(mut self, validator: Arc<dyn OutputValidator>) -> Self {
        self.output_validators.push(validator);
        self
    }

    /// Set usage limits overlay.
    #[must_use]
    pub const fn with_usage_limits(mut self, limits: UsageLimits) -> Self {
        self.usage_limits = Some(limits);
        self
    }
}

impl CapabilityBundle for StaticCapabilityBundle {
    fn name(&self) -> &str {
        &self.name
    }

    fn spec(&self) -> CapabilitySpec {
        self.spec
            .clone()
            .unwrap_or_else(|| CapabilitySpec::new(self.name.clone()))
    }

    fn hooks(&self) -> Vec<Arc<dyn AgentCapability>> {
        self.hooks.clone()
    }

    fn stream_observers(&self) -> Vec<Arc<dyn AgentCapability>> {
        self.stream_observers.clone()
    }

    fn get_instructions(&self) -> Vec<String> {
        self.instructions.clone()
    }

    fn dynamic_instructions(&self) -> Vec<DynDynamicInstruction> {
        self.dynamic_instructions.clone()
    }

    fn get_tools(&self) -> Option<ToolRegistry> {
        if self.tools.is_empty() && self.tools.get_instructions().is_empty() {
            None
        } else {
            Some(self.tools.clone())
        }
    }

    fn model_settings(&self) -> Option<ModelSettings> {
        self.model_settings.clone()
    }

    fn request_params(&self) -> Option<ModelRequestParameters> {
        self.request_params.clone()
    }

    fn output_functions(&self) -> Vec<DynOutputFunction> {
        self.output_functions.clone()
    }

    fn output_validators(&self) -> Vec<Arc<dyn OutputValidator>> {
        self.output_validators.clone()
    }

    fn usage_limits(&self) -> Option<UsageLimits> {
        self.usage_limits.clone()
    }
}
