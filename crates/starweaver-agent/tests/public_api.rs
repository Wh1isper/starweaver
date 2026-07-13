#![allow(missing_docs)]

use starweaver_agent::{advanced, prelude::*};

#[test]
fn stable_prelude_and_advanced_namespaces_are_available() {
    fn accepts_model_settings(_: ModelSettings) {}
    fn accepts_runtime_policy(_: advanced::runtime::AgentRuntimePolicy) {}
    fn accepts_session_status(_: advanced::session::SessionStatus) {}
    fn accepts_stream_scope(_: advanced::stream::ReplayScope) {}
    fn accepts_context(_: advanced::context::AgentContext) {}
    fn accepts_tool_registry(_: advanced::tools::ToolRegistry) {}
    fn assert_type<T>() {}

    accepts_model_settings(ModelSettings::default());
    accepts_runtime_policy(advanced::runtime::AgentRuntimePolicy::default());
    accepts_session_status(advanced::session::SessionStatus::Active);
    accepts_stream_scope(advanced::stream::ReplayScope::run("run-public-api"));
    accepts_context(advanced::context::AgentContext::default());
    accepts_tool_registry(advanced::tools::ToolRegistry::new());
    assert_type::<advanced::model::ModelMessage>();
}
