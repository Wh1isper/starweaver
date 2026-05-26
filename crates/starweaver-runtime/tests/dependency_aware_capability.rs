//! Dependency-aware capability hook tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_context::{AgentContext, BusMessage};
use starweaver_model::{ModelRequest, ModelResponse, ModelSettings, TestModel};
use starweaver_runtime::{Agent, AgentCapability, AgentRunState, CapabilityResult};

#[derive(Clone, Debug)]
struct Tenant(String);

struct ContextAwareCapability {
    observed: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentCapability for ContextAwareCapability {
    async fn on_run_start_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        let tenant = context
            .dependency::<Tenant>()
            .map_or_else(|| "missing".to_string(), |tenant| tenant.0.clone());
        if let Ok(mut observed) = self.observed.lock() {
            observed.push(format!("start:{tenant}"));
        }
        context.enqueue_message(BusMessage::new(
            "capability.started",
            serde_json::json!({"tenant": tenant}),
        ));
        Ok(())
    }

    async fn before_model_request_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        request: &mut ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        let tenant = context
            .dependency::<Tenant>()
            .map_or_else(|| "missing".to_string(), |tenant| tenant.0.clone());
        request
            .metadata
            .insert("tenant".to_string(), serde_json::json!(tenant));
        Ok(())
    }

    async fn after_model_response_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        response: &mut ModelResponse,
    ) -> CapabilityResult<()> {
        let tenant = context
            .dependency::<Tenant>()
            .map_or_else(|| "missing".to_string(), |tenant| tenant.0.clone());
        response
            .metadata
            .insert("tenant".to_string(), serde_json::json!(tenant));
        Ok(())
    }

    async fn on_run_complete_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        if let Ok(mut observed) = self.observed.lock() {
            observed.push(format!("messages:{}", context.messages.len()));
        }
        Ok(())
    }
}

#[tokio::test]
async fn capability_hooks_can_read_context_dependencies() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let capability = Arc::new(ContextAwareCapability {
        observed: observed.clone(),
    });
    let model = Arc::new(TestModel::with_text("ok"));
    let mut context = AgentContext::default();
    context.insert_dependency(Tenant("acme".to_string()));

    let result = Agent::new(model)
        .with_capability(capability)
        .run_with_context("hello", &mut context)
        .await;

    assert!(result.is_ok());
    assert_eq!(
        observed
            .lock()
            .map_or_else(|_| Vec::new(), |observed| observed.clone()),
        vec!["start:acme".to_string(), "messages:1".to_string()]
    );
    assert_eq!(context.messages.len(), 1);
}
