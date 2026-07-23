//! Canonical event replay request coverage derived from `protocol/host`.
#![allow(clippy::expect_used)]

use starweaver_rpc_core::generated::{HostCall, decode_request_frame};

const EVENTS_REPLAY_REQUEST: &str =
    include_str!("../../../protocol/host/examples/events-replay.request.json");

#[test]
fn canonical_events_replay_example_decodes_through_generated_boundary() {
    let request = decode_request_frame(EVENTS_REPLAY_REQUEST.as_bytes())
        .expect("canonical events.replay request");
    let HostCall::EventsReplay(params) = request.call else {
        panic!("canonical example decoded as the wrong host call");
    };

    assert_eq!(request.id.as_str(), "req_3");
    assert_eq!(params.limit, 100);
    assert!(params.cursor.is_none());
    assert!(params.view.optional_features.is_empty());
}
