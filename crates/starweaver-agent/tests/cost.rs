#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, CostBudget, TestModel, UsageLimits};
use starweaver_model::ModelResponse;
use starweaver_usage::Usage;

#[tokio::test]
async fn builder_applies_cost_budget_usage_limits() {
    let model = Arc::new(TestModel::with_responses(vec![ModelResponse {
        usage: Usage {
            requests: 1,
            input_tokens: 10,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 0,
            total_tokens: 10,
            tool_calls: 0,
        },
        ..ModelResponse::text("ok")
    }]));

    let result = AgentBuilder::new(model)
        .usage_limits(
            UsageLimits::new().with_cost_budget(
                CostBudget::new()
                    .with_input_micros_per_million_tokens(1_000_000)
                    .with_total_cost_limit_micros(10),
            ),
        )
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}
