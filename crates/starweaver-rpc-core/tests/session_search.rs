#![allow(missing_docs, clippy::expect_used)]

use serde_json::json;
use starweaver_rpc_core::{
    SESSION_SEARCH_FEATURE, SessionSearchParams, host_protocol_identity,
    host_protocol_identity_with_session_search,
};
use starweaver_session::{SessionSearchGranularity, SessionSearchSource, SessionStatus};

#[test]
fn session_search_request_projects_host_casing_to_domain_query() {
    let params: SessionSearchParams = serde_json::from_value(json!({
        "query": "oauth refresh",
        "filters": {
            "status": ["active"],
            "workspace": "/workspace/project"
        },
        "sources": ["session_metadata", "run_input"],
        "granularity": "run",
        "limit": 17,
        "cursor": null
    }))
    .expect("typed params");
    let query = params.into_query();
    assert_eq!(query.text.as_deref(), Some("oauth refresh"));
    assert_eq!(query.filter.session_statuses, vec![SessionStatus::Active]);
    assert_eq!(
        query.filter.workspace.as_deref(),
        Some("/workspace/project")
    );
    assert!(
        query
            .sources
            .contains(&SessionSearchSource::SessionMetadata)
    );
    assert!(query.sources.contains(&SessionSearchSource::RunInput));
    assert_eq!(query.granularity, SessionSearchGranularity::Run);
    assert_eq!(query.limit, 17);
}

#[test]
fn request_cannot_supply_authorization_scope() {
    let error = serde_json::from_value::<SessionSearchParams>(json!({
        "query": "hidden",
        "tenant": "other-tenant",
        "limit": 20
    }))
    .expect_err("unknown tenant authority must fail");
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn protocol_advertises_search_only_when_installed() {
    assert!(
        !host_protocol_identity()
            .features
            .iter()
            .any(|feature| feature == SESSION_SEARCH_FEATURE)
    );
    assert!(
        host_protocol_identity_with_session_search(true)
            .features
            .iter()
            .any(|feature| feature == SESSION_SEARCH_FEATURE)
    );
}
