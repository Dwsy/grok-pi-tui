use serde_json::{Value, json};

use crate::{ManagedGatewayCall, MockInferenceServer};

async fn list(server: &MockInferenceServer) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}/mcp/tools/list", server.url()))
        .send()
        .await
        .expect("GET /v1/mcp/tools/list")
}

async fn call(server: &MockInferenceServer, body: &Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/mcp/tools/call", server.url()))
        .header("authorization", "Bearer gateway-token")
        .json(body)
        .send()
        .await
        .expect("POST /v1/mcp/tools/call")
}

#[tokio::test]
async fn unscripted_gateway_answers_404_and_records_nothing() {
    let server = MockInferenceServer::start().await.unwrap();

    assert_eq!(reqwest::StatusCode::NOT_FOUND, list(&server).await.status());
    let response = call(&server, &json!({ "call_id": "x", "arguments": {} })).await;

    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
    assert_eq!(
        Vec::<ManagedGatewayCall>::new(),
        server.managed_gateway_calls()
    );
}

#[tokio::test]
async fn scripted_gateway_serves_catalog_and_records_calls() {
    let server = MockInferenceServer::start().await.unwrap();
    let catalog = json!({ "tools": [{ "connector_id": "c", "tool_id": "t" }], "total_tools": 1 });
    let result = json!({ "content": [{ "type": "text", "text": "MARKER" }] });
    server.set_managed_gateway(catalog.clone(), result.clone());

    let listed = list(&server).await;
    assert_eq!(reqwest::StatusCode::OK, listed.status());
    assert_eq!(catalog, listed.json::<Value>().await.unwrap());
    let answered = call(
        &server,
        &json!({ "call_id": "c.t", "arguments": { "q": 1 } }),
    )
    .await;

    assert_eq!(reqwest::StatusCode::OK, answered.status());
    assert_eq!(
        json!({ "result": result, "connectors_needing_reauth": [] }),
        answered.json::<Value>().await.unwrap()
    );
    assert_eq!(
        vec![ManagedGatewayCall {
            call_id: "c.t".to_owned(),
            arguments: json!({ "q": 1 }),
            authorization: Some("Bearer gateway-token".to_owned()),
        }],
        server.managed_gateway_calls()
    );
    assert_eq!(1, server.request_count_for("/v1/mcp/tools/list"));
}
