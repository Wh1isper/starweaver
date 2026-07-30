//! Media policy, preflight, compression, and upload filters.

mod capability;
mod compression;
mod geometry;
mod policy;
mod preflight;
mod split;
mod upload;

pub(super) use capability::capability_filter;
pub(super) use compression::media_compress_filter;
pub(super) use geometry::geometry_media_admission_filter;
pub(super) use preflight::media_preflight_filter;
pub(super) use split::media_split_filter;
pub(super) use upload::media_upload_filter;
pub use upload::{MediaUploadRequest, MediaUploader};

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use serde_json::json;
    use starweaver_context::{AgentContext, ModelCapability};
    use starweaver_core::{ConversationId, RunId};
    use starweaver_model::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart};
    use starweaver_runtime::AgentRunState;

    use super::{
        MediaUploadRequest, MediaUploader, capability_filter, media_compress_filter,
        media_preflight_filter, media_split_filter, media_upload_filter,
    };

    #[derive(Default)]
    struct CountingUploader {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl MediaUploader for CountingUploader {
        async fn upload(&self, _request: MediaUploadRequest) -> Result<ContentPart, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ContentPart::ImageUrl {
                url: "https://example.invalid/uploaded.png".to_owned(),
            })
        }
    }

    fn geometry_bound_request() -> Vec<ModelMessage> {
        vec![ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Binary {
                    data: vec![0, 1, 2, 3],
                    media_type: "image/png".to_owned(),
                }],
                name: None,
                metadata: serde_json::Map::from_iter([(
                    "starweaver_geometry_bound_immutable_media".to_owned(),
                    json!(true),
                )]),
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })]
    }

    #[tokio::test]
    async fn geometry_bound_media_bypasses_every_generic_media_transform() {
        let state = AgentRunState::new(
            RunId::from_string("run-geometry-media"),
            ConversationId::from_string("conversation-geometry-media"),
        );
        let mut context = AgentContext::default();
        context.model_config.max_image_bytes = 1;
        context.model_config.max_image_dimension = 1;
        context.model_config.max_images = 0;
        context.model_config.split_large_images = true;
        context.model_config.image_split_max_height = 1;
        context
            .model_config
            .capabilities
            .insert(ModelCapability::ImageUrl);
        let original = geometry_bound_request();

        assert_eq!(
            capability_filter(&state, &context, original.clone()),
            original
        );
        assert_eq!(
            media_compress_filter(&state, &context, original.clone()),
            original
        );
        assert_eq!(
            media_preflight_filter(&state, &context, original.clone()),
            original
        );
        assert_eq!(
            media_split_filter(&state, &context, original.clone()),
            original
        );

        let uploader = Arc::new(CountingUploader::default());
        let uploader_dyn: Arc<dyn MediaUploader> = uploader.clone();
        assert_eq!(
            media_upload_filter(&state, &context, original.clone(), Some(&uploader_dyn),).await,
            original
        );
        assert_eq!(uploader.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn immutable_geometry_media_reserves_the_total_image_count_budget() {
        let state = AgentRunState::new(
            RunId::from_string("run-geometry-count"),
            ConversationId::from_string("conversation-geometry-count"),
        );
        let mut context = AgentContext::default();
        context.model_config.max_images = 1;
        let mut messages = geometry_bound_request();
        let ModelMessage::Request(request) = &mut messages[0] else {
            panic!("fixture should contain one request");
        };
        request.parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::ImageUrl {
                url: "https://example.invalid/newer.png".to_owned(),
            }],
            name: None,
            metadata: serde_json::Map::new(),
        });

        let filtered = media_preflight_filter(&state, &context, messages);
        let ModelMessage::Request(request) = &filtered[0] else {
            panic!("filtered history should contain one request");
        };
        let ModelRequestPart::UserPrompt {
            content: geometry_content,
            ..
        } = &request.parts[0]
        else {
            panic!("geometry prompt should remain first");
        };
        assert!(matches!(
            &geometry_content[0],
            ContentPart::Binary { data, .. } if data == &[0, 1, 2, 3]
        ));
        let ModelRequestPart::UserPrompt {
            content: ordinary_content,
            ..
        } = &request.parts[1]
        else {
            panic!("ordinary prompt should remain second");
        };
        assert!(matches!(
            &ordinary_content[0],
            ContentPart::Text { text } if text.contains("max_images=1")
        ));
    }
}
