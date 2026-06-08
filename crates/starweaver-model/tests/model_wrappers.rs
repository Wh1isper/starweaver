#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use serde_json::json;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ConcurrencyLimitedModel, FallbackModel, FunctionModel, FunctionModelInfo, ModelAdapter,
    ModelError, ModelMessage, ModelProfile, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ProfileOverrideModel, ProtocolFamily,
};
use tokio::sync::{oneshot, Mutex};

fn context() -> ModelRequestContext {
    ModelRequestContext::new(RunId::default(), ConversationId::default())
}

#[tokio::test]
async fn fallback_model_records_selected_model_and_failures() {
    let failing = Arc::new(FunctionModel::new(|_messages, _settings, info| {
        assert_eq!(
            info.context.llm_trace_metadata["starweaver_model_wrapper"]["attempt"],
            json!(1)
        );
        Err(ModelError::Transport("primary unavailable".to_string()))
    })) as Arc<dyn ModelAdapter>;
    let backup = Arc::new(
        FunctionModel::new(|_messages, _settings, info| {
            assert_eq!(
                info.context.llm_trace_metadata["starweaver_model_wrapper"]["attempt"],
                json!(2)
            );
            Ok(ModelResponse::text("backup result"))
        })
        .with_model_name("backup-model"),
    ) as Arc<dyn ModelAdapter>;

    let model = FallbackModel::new(vec![failing, backup]);
    let response = model
        .request(
            vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), "backup result");
    let wrapper = &response.metadata["starweaver_model_wrapper"];
    assert_eq!(wrapper["kind"], "fallback");
    assert_eq!(wrapper["selected_attempt"], 2);
    assert_eq!(wrapper["selected_model"], "backup-model");
    assert!(wrapper["failures"][0]["error"]
        .as_str()
        .unwrap()
        .contains("primary unavailable"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrency_limited_model_holds_second_request_until_first_finishes() {
    #[derive(Clone)]
    struct GateState {
        started: Arc<AtomicUsize>,
        release_first: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    }

    let (release_tx, release_rx) = oneshot::channel();
    let state = GateState {
        started: Arc::new(AtomicUsize::new(0)),
        release_first: Arc::new(Mutex::new(Some(release_rx))),
    };
    let model_state = state.clone();
    let inner = Arc::new(FunctionModel::new(move |_messages, _settings, info| {
        assert_eq!(
            info.context.llm_trace_metadata["starweaver_model_wrapper"]["kind"],
            "concurrency_limited"
        );
        let state = model_state.clone();
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async move {
                let order = state.started.fetch_add(1, Ordering::SeqCst) + 1;
                if order == 1 {
                    let rx = state.release_first.lock().await.take().unwrap();
                    let _ = rx.await;
                }
                Ok(ModelResponse::text(format!("done-{order}")))
            })
        })
    })) as Arc<dyn ModelAdapter>;
    let limited = Arc::new(ConcurrencyLimitedModel::new(inner, 1));

    let first_model = limited.clone();
    let first = tokio::spawn(async move {
        first_model
            .request(
                vec![ModelMessage::Request(ModelRequest::user_text("first"))],
                None,
                ModelRequestParameters::default(),
                context(),
            )
            .await
            .unwrap()
    });
    while state.started.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    let second_model = limited.clone();
    let second = tokio::spawn(async move {
        second_model
            .request(
                vec![ModelMessage::Request(ModelRequest::user_text("second"))],
                None,
                ModelRequestParameters::default(),
                context(),
            )
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;
    assert_eq!(state.started.load(Ordering::SeqCst), 1);

    release_tx.send(()).unwrap();
    let first_response = first.await.unwrap();
    let second_response = second.await.unwrap();

    assert_eq!(first_response.text_output(), "done-1");
    assert_eq!(second_response.text_output(), "done-2");
}

#[test]
fn profile_override_exposes_replacement_profile_and_settings() {
    let base = Arc::new(FunctionModel::new(
        |_messages, _settings, _info: FunctionModelInfo| Ok(ModelResponse::text("ok")),
    )) as Arc<dyn ModelAdapter>;
    let profile = ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages);
    let wrapper = ProfileOverrideModel::new(base, profile.clone()).with_model_name("profiled");

    assert_eq!(wrapper.model_name(), "profiled");
    assert_eq!(wrapper.profile(), &profile);
}
