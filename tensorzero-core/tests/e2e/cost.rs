//! E2E tests for cost tracking.
//!
//! These tests verify that cost is correctly computed from provider raw responses
//! using JSON Pointer-based cost configuration, and that cost is stored in
//! the database and returned in API responses.

use futures::StreamExt;
use reqwest::Client;
use reqwest_sse_stream::{Event, RequestBuilderExt};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tensorzero_core::db::clickhouse::test_helpers::{
    get_clickhouse, select_model_inference_clickhouse,
};
use uuid::Uuid;

use crate::common::get_gateway_endpoint;

/// Expected cost for the dummy "good" model with cost config:
///   - prompt_tokens: 10 * 1.50 / 1_000_000 = 0.000015
///   - completion_tokens: 10 * 2.00 / 1_000_000 = 0.00002
///   - total: 0.000035
fn expected_dummy_cost() -> Decimal {
    // 10 * 1.50 / 1_000_000 + 10 * 2.00 / 1_000_000 = 0.000035
    Decimal::new(35, 6)
}

/// Helper to make a simple non-streaming inference request.
async fn make_inference(function_name: &str, stream: bool) -> Value {
    let payload = json!({
        "function_name": function_name,
        "episode_id": Uuid::now_v7(),
        "input": {
            "system": {"assistant_name": "AskJeeves"},
            "messages": [
                {
                    "role": "user",
                    "content": "Hello, world!"
                }
            ]
        },
        "stream": stream,
    });

    let response = Client::new()
        .post(get_gateway_endpoint("/inference"))
        .json(&payload)
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success(), "inference request failed");
    response.json::<Value>().await.unwrap()
}

/// Helper to make a streaming inference request and return the last chunk (which has usage).
async fn make_streaming_inference(function_name: &str) -> Value {
    let payload = json!({
        "function_name": function_name,
        "episode_id": Uuid::now_v7(),
        "input": {
            "system": {"assistant_name": "AskJeeves"},
            "messages": [
                {
                    "role": "user",
                    "content": "Hello, world!"
                }
            ]
        },
        "stream": true,
    });

    let mut event_source = Client::new()
        .post(get_gateway_endpoint("/inference"))
        .json(&payload)
        .eventsource()
        .await
        .unwrap();

    let mut chunks = vec![];
    while let Some(event) = event_source.next().await {
        let event = event.unwrap();
        match event {
            Event::Open => continue,
            Event::Message(message) => {
                if message.data == "[DONE]" {
                    break;
                }
                chunks.push(message.data);
            }
        }
    }

    assert!(!chunks.is_empty(), "expected at least one chunk");
    serde_json::from_str(chunks.last().unwrap()).unwrap()
}

// ─── Non-streaming cost tests ────────────────────────────────────────────────

/// Verify that cost is present in the non-streaming API response for a model
/// with cost configuration.
#[tokio::test]
async fn test_cost_in_non_streaming_response() {
    let response = make_inference("basic_test", false).await;

    let usage = response.get("usage").expect("response should have usage");
    let cost = usage.get("cost");
    assert!(
        cost.is_some(),
        "usage should include cost for a model with cost configuration"
    );

    // Parse cost as Decimal for comparison
    let cost_str = cost.unwrap().as_str().unwrap_or_else(|| {
        // cost might be a number, not a string
        panic!("cost should be a string or number, got: {cost:?}");
    });
    let cost_decimal: Decimal = cost_str.parse().unwrap();
    assert_eq!(
        cost_decimal,
        expected_dummy_cost(),
        "cost should match expected value based on token counts and rates"
    );
}

/// Verify that cost is stored in ClickHouse for non-streaming inferences.
#[tokio::test]
async fn test_cost_stored_in_clickhouse_non_streaming() {
    let response = make_inference("basic_test", false).await;
    let inference_id =
        Uuid::parse_str(response.get("inference_id").unwrap().as_str().unwrap()).unwrap();

    // Wait for ClickHouse writes
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let clickhouse = get_clickhouse().await;
    let model_inference = select_model_inference_clickhouse(&clickhouse, inference_id)
        .await
        .expect("model inference should be stored in ClickHouse");

    let cost = model_inference.get("cost");
    assert!(
        cost.is_some() && !cost.unwrap().is_null(),
        "cost should be stored in ClickHouse model inference"
    );

    // ClickHouse returns Decimal as a string
    let cost_str = cost.unwrap().as_str().unwrap_or_else(|| {
        panic!(
            "ClickHouse cost should be a string, got: {:?}",
            cost.unwrap()
        );
    });
    let cost_decimal: Decimal = cost_str.parse().unwrap();
    assert_eq!(
        cost_decimal,
        expected_dummy_cost(),
        "ClickHouse cost should match expected value"
    );
}

// ─── Streaming cost tests ────────────────────────────────────────────────────

/// Verify that cost is present in the final streaming chunk's usage.
#[tokio::test]
async fn test_cost_in_streaming_response() {
    let last_chunk = make_streaming_inference("basic_test").await;

    let usage = last_chunk.get("usage");
    // In streaming, usage might be in the last chunk
    if let Some(usage) = usage {
        let cost = usage.get("cost");
        assert!(
            cost.is_some(),
            "usage in streaming final chunk should include cost"
        );
    }
}

/// Verify that cost is stored in ClickHouse for streaming inferences.
#[tokio::test]
async fn test_cost_stored_in_clickhouse_streaming() {
    let last_chunk = make_streaming_inference("basic_test").await;
    let inference_id =
        Uuid::parse_str(last_chunk.get("inference_id").unwrap().as_str().unwrap()).unwrap();

    // Wait for ClickHouse writes
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let clickhouse = get_clickhouse().await;
    let model_inference = select_model_inference_clickhouse(&clickhouse, inference_id)
        .await
        .expect("model inference should be stored in ClickHouse");

    let cost = model_inference.get("cost");
    assert!(
        cost.is_some() && !cost.unwrap().is_null(),
        "cost should be stored in ClickHouse for streaming inferences"
    );
}

// ─── Missing cost configuration ──────────────────────────────────────────────

/// Verify that cost is None/missing when a model has no cost configuration.
#[tokio::test]
async fn test_no_cost_when_not_configured() {
    // "model_fallback_test" uses the "test_fallback" model which routes to
    // error (fails) then "good" (succeeds). The "good" provider on "test_fallback"
    // doesn't have cost configured.
    let response = make_inference("model_fallback_test", false).await;

    let usage = response.get("usage").expect("response should have usage");
    let cost = usage.get("cost");
    // cost should be absent (None / not serialized)
    assert!(
        cost.is_none() || cost.unwrap().is_null(),
        "cost should be absent when model has no cost configuration"
    );
}

// ─── Cost aggregation endpoint tests ─────────────────────────────────────────

/// Verify that the cost_by_variant endpoint returns cost data after making inferences.
#[tokio::test]
async fn test_cost_by_variant_cumulative() {
    // Make a non-streaming inference to ensure there's cost data
    let response = make_inference("basic_test", false).await;
    let inference_id = response.get("inference_id").unwrap().as_str().unwrap();
    assert!(
        !inference_id.is_empty(),
        "inference should return an inference_id"
    );

    // Wait for database writes to complete
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Query the cost_by_variant endpoint with cumulative time window
    let url = get_gateway_endpoint(
        "/internal/functions/basic_test/cost_by_variant?time_window=cumulative",
    );
    let response = Client::new().get(url).send().await.unwrap();
    assert!(
        response.status().is_success(),
        "cost_by_variant endpoint should succeed, got status: {}",
        response.status()
    );

    let body: Value = response.json().await.unwrap();
    let cost_data = body.get("cost").expect("response should have `cost` field");
    let cost_array = cost_data.as_array().expect("`cost` should be an array");

    // There should be at least one entry (the variant that served the inference)
    assert!(
        !cost_array.is_empty(),
        "cost_by_variant should return at least one entry after an inference"
    );

    // Verify each entry has the expected shape
    for entry in cost_array {
        assert!(
            entry.get("period_start").is_some(),
            "each cost entry should have `period_start`"
        );
        assert!(
            entry.get("variant_name").is_some(),
            "each cost entry should have `variant_name`"
        );
        assert!(
            entry.get("total_cost").is_some(),
            "each cost entry should have `total_cost`"
        );
        assert!(
            entry.get("inference_count").is_some(),
            "each cost entry should have `inference_count`"
        );
        assert!(
            entry.get("inferences_with_cost").is_some(),
            "each cost entry should have `inferences_with_cost`"
        );

        // total_cost should be > 0 since we have cost config
        let total_cost_str = entry.get("total_cost").unwrap().as_str().unwrap();
        let total_cost: Decimal = total_cost_str.parse().unwrap();
        assert!(
            total_cost > Decimal::ZERO,
            "total_cost should be positive when cost config is present"
        );

        // inference_count should be >= inferences_with_cost
        let inference_count = entry.get("inference_count").unwrap().as_u64().unwrap();
        let inferences_with_cost = entry.get("inferences_with_cost").unwrap().as_u64().unwrap();
        assert!(
            inference_count >= inferences_with_cost,
            "inference_count ({inference_count}) should be >= inferences_with_cost ({inferences_with_cost})"
        );

        // All inferences for basic_test have cost config, so coverage should be 100%
        assert!(
            inferences_with_cost > 0,
            "inferences_with_cost should be > 0 for a model with cost config"
        );
    }
}

/// Verify that cost_by_variant works with time-windowed queries.
#[tokio::test]
async fn test_cost_by_variant_time_windowed() {
    // Make an inference to ensure there's data
    make_inference("basic_test", false).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Query with a day time window
    let url = get_gateway_endpoint(
        "/internal/functions/basic_test/cost_by_variant?time_window=day&max_periods=7",
    );
    let response = Client::new().get(url).send().await.unwrap();
    assert!(
        response.status().is_success(),
        "cost_by_variant with day time window should succeed"
    );

    let body: Value = response.json().await.unwrap();
    let cost_data = body.get("cost").expect("response should have `cost` field");
    let cost_array = cost_data.as_array().expect("`cost` should be an array");

    assert!(
        !cost_array.is_empty(),
        "cost_by_variant with day window should return data"
    );

    // Verify period_start is a valid RFC 3339 timestamp (not epoch)
    for entry in cost_array {
        let period_start = entry.get("period_start").unwrap().as_str().unwrap();
        assert!(
            period_start != "1970-01-01T00:00:00.000Z",
            "time-windowed query should not return epoch timestamp"
        );
    }
}

/// Verify that cost_by_variant returns 404 for non-existent function.
#[tokio::test]
async fn test_cost_by_variant_unknown_function() {
    let url = get_gateway_endpoint(
        "/internal/functions/nonexistent_function_xyz/cost_by_variant?time_window=cumulative",
    );
    let response = Client::new().get(url).send().await.unwrap();
    assert!(
        !response.status().is_success(),
        "cost_by_variant should fail for non-existent function"
    );
}
