use std::{collections::BTreeSet, sync::Arc};

use serde_json::json;
use starweaver_computer_use::{
    COMPUTER_STATUS_TOOL, ComputerSessionBinding, ComputerToolCatalog, ComputerToolContent,
    ComputerToolGrant, ComputerToolInvocation, ComputerToolRouter, ComputerUsePolicy,
    DesktopImageMime, FakeComputerUseConfig, FakeComputerUseService, InvocationId,
    InvocationSource,
};
use starweaver_context::{AgentContext, ModelCapability};
use starweaver_core::{AgentId, CancellationToken, ConversationId, RunId};
use starweaver_tools::{
    ToolContext, ToolDependencyProfile, ToolError, tool_dependency_requirements,
};

use super::{
    COMPUTER_OBSERVE_CAPABILITY, ComputerUseAdmissionGuard, ComputerUseAttachmentError,
    ComputerUseToolsetPolicy, attach_computer_use, attach_guarded_computer_use, computer_use_tools,
    validate_model_image_limits,
};

fn fake_router(grant: ComputerToolGrant) -> Arc<ComputerToolRouter> {
    let service = Arc::new(FakeComputerUseService::new(
        ComputerUsePolicy::default(),
        FakeComputerUseConfig::default(),
    ));
    Arc::new(ComputerToolRouter::new(
        service,
        ComputerSessionBinding::ServiceOwnedLazy,
        grant,
    ))
}

#[test]
fn attachment_rejects_input_without_observe() {
    let mut context = AgentContext::new(AgentId::from_string("computer-use-test"));
    let grant = ComputerToolGrant {
        observe: false,
        pointer: true,
        keyboard: false,
    };

    assert_eq!(
        attach_computer_use(&mut context, fake_router(grant), grant),
        Err(ComputerUseAttachmentError::InputRequiresObserve)
    );
    assert!(
        context
            .named_dependency::<super::ComputerPointerHandle>(super::COMPUTER_POINTER_CAPABILITY)
            .is_none()
    );
}

#[tokio::test]
async fn preparation_requires_attached_grant_intersected_handles() {
    let grant = ComputerToolGrant::observe_only();
    let toolset = computer_use_tools(grant, ComputerUseToolsetPolicy::default());
    let mut context = AgentContext::new(AgentId::from_string("computer-use-test"));

    let unavailable = toolset.prepare_with_context(&context).await;
    let Ok(unavailable) = unavailable else {
        panic!("unattached preparation should return a lifecycle report");
    };
    assert!(unavailable.tools.is_empty());

    let attached = attach_computer_use(&mut context, fake_router(grant), grant);
    assert_eq!(attached, Ok(()));
    let prepared = toolset.prepare_with_context(&context).await;
    let Ok(prepared) = prepared else {
        panic!("attached preparation should succeed");
    };
    assert_eq!(
        prepared
            .tools
            .iter()
            .map(|tool| tool.name())
            .collect::<Vec<_>>(),
        ["computer_status", "computer_observe"]
    );
    for tool in &prepared.tools {
        let requirements = tool_dependency_requirements(&tool.metadata());
        assert_eq!(requirements.profile, ToolDependencyProfile::Strict);
        assert_eq!(
            requirements.host_capabilities.iter().collect::<Vec<_>>(),
            [COMPUTER_OBSERVE_CAPABILITY]
        );
        assert!(!requirements.shell_environment);
    }
}

#[tokio::test]
async fn guarded_handle_fails_closed_after_process_local_revocation() {
    let grant = ComputerToolGrant::observe_only();
    let revoked = CancellationToken::new();
    let guard = ComputerUseAdmissionGuard::with_revocation(|| true, revoked.clone());
    let mut context = AgentContext::new(AgentId::from_string("computer-use-test"));
    assert_eq!(
        attach_guarded_computer_use(&mut context, fake_router(grant), grant, guard),
        Ok(())
    );
    let Some(handle) =
        context.named_dependency::<super::ComputerObserveHandle>(COMPUTER_OBSERVE_CAPABILITY)
    else {
        panic!("observe handle should be attached");
    };
    revoked.cancel();
    let result = handle
        .status(
            ComputerToolInvocation::new(
                InvocationId::from_stable_parts("test", ["run", "call"]),
                InvocationSource::StarweaverToolCall,
            ),
            json!({}),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_error);
    let Some(error) = result.structured.error else {
        panic!("revocation should return a structured error");
    };
    assert_eq!(error.code, "policy_denied");
}

#[test]
fn toolset_schemas_are_the_canonical_catalog_and_not_core_registered() {
    let grant = ComputerToolGrant {
        observe: true,
        pointer: true,
        keyboard: true,
    };
    let tools = computer_use_tools(grant, ComputerUseToolsetPolicy::default()).get_tools();
    let canonical = ComputerToolCatalog::definitions(grant);

    assert_eq!(tools.len(), canonical.len());
    for (tool, definition) in tools.iter().zip(canonical) {
        assert_eq!(tool.name(), definition.name);
        assert_eq!(tool.parameters_schema(), definition.input_schema);
        assert_eq!(tool.return_schema(), Some(definition.output_schema));
    }
    assert!(
        crate::bundles::core_toolsets()
            .iter()
            .all(|toolset| toolset.id() != Some(starweaver_computer_use::COMPUTER_USE_TOOLSET_ID))
    );
}

#[tokio::test]
async fn dispatch_requires_exact_runtime_tool_call_id() {
    let grant = ComputerToolGrant::observe_only();
    let mut context = AgentContext::new(AgentId::from_string("computer-use-test"));
    assert_eq!(
        attach_computer_use(&mut context, fake_router(grant), grant),
        Ok(())
    );
    let toolset = computer_use_tools(grant, ComputerUseToolsetPolicy::default());
    let Some(tool) = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == COMPUTER_STATUS_TOOL)
    else {
        panic!("status tool should exist");
    };
    let requirements = tool_dependency_requirements(&tool.metadata());
    let assembly = starweaver_runtime::assemble_tool_dependencies_for_name(
        &context,
        tool.name(),
        &requirements,
        &context.tool_capability_grant(tool.name()),
    );
    let tool_context = ToolContext::new(
        RunId::from_string("run-computer-use"),
        ConversationId::from_string("conversation-computer-use"),
        0,
    )
    .with_dependencies(assembly.dependencies);

    let error = tool.call(tool_context, json!({})).await;
    assert!(matches!(
        error,
        Err(ToolError::Execution { message, .. })
            if message.contains("stable tool_call_id is required")
    ));
}

fn image_tool_context(context: &AgentContext) -> ToolContext {
    ToolContext::new(
        RunId::from_string("run-computer-use-media"),
        ConversationId::from_string("conversation-computer-use-media"),
        0,
    )
    .with_dependencies(context.strict_tool_dependency_store(&BTreeSet::new(), false))
}

fn image_content(bytes: Vec<u8>) -> ComputerToolContent {
    ComputerToolContent::Image {
        mime_type: DesktopImageMime::ImagePng,
        bytes,
        width: 1,
        height: 1,
        sha256: "digest".to_owned(),
        observation_id: "observation-1".to_owned(),
    }
}

#[test]
fn exact_image_admission_requires_active_vision_and_nonzero_count_budget() {
    let mut context = AgentContext::default();
    context.model_config.max_images = 1;
    context.model_config.max_image_bytes = 64;
    let items = vec![image_content(vec![1, 2, 3, 4])];

    let Err(no_vision) = validate_model_image_limits(&image_tool_context(&context), &items) else {
        panic!("a model without image capability must reject the screenshot");
    };
    assert!(matches!(
        no_vision,
        ToolError::UserError { message, .. }
            if message.contains("safety admission rejected")
                && message.contains("does not advertise image capability")
    ));

    context
        .model_config
        .capabilities
        .insert(ModelCapability::Vision);
    context.model_config.max_images = 0;
    let Err(no_count) = validate_model_image_limits(&image_tool_context(&context), &items) else {
        panic!("a zero image-count budget must reject the screenshot");
    };
    assert!(matches!(
        no_count,
        ToolError::UserError { message, .. }
            if message.contains("max_images=0")
                && message.contains("not transformed or submitted")
    ));
}

#[test]
fn exact_image_admission_uses_base64_single_and_aggregate_byte_hard_limits() {
    let mut context = AgentContext::default();
    context
        .model_config
        .capabilities
        .insert(ModelCapability::Vision);
    context.model_config.max_images = 2;
    context.model_config.max_image_bytes = 7;

    let Err(single) = validate_model_image_limits(
        &image_tool_context(&context),
        &[image_content(vec![1, 2, 3, 4])],
    ) else {
        panic!("an oversized exact screenshot must be rejected");
    };
    assert!(matches!(
        single,
        ToolError::UserError { message, .. }
            if message.contains("requires 8 base64 bytes")
                && message.contains("max_image_bytes=7")
    ));

    let Err(aggregate) = validate_model_image_limits(
        &image_tool_context(&context),
        &[image_content(vec![1, 2, 3]), image_content(vec![4, 5, 6])],
    ) else {
        panic!("an aggregate image-byte overflow must be rejected");
    };
    assert!(matches!(
        aggregate,
        ToolError::UserError { message, .. }
            if message.contains("8 total base64 image bytes")
                && message.contains("hard aggregate limit")
    ));
}
