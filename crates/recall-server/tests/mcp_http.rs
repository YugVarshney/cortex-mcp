//! MCP conformance smoke tests over the streamable-HTTP transport:
//! initialize -> tools/list -> tools/call, plus API-key behavior on /mcp.

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use recall_core::shared;
use recall_server::{ServerConfig, build_router};
use serde_json::{Value, json};
use tower::ServiceExt;

fn app_with(api_key: Option<String>) -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = shared(recall_core::SqliteStore::open(dir.path().join("mcp.db")).expect("db"));
    std::mem::forget(dir);
    build_router(
        store,
        &ServerConfig {
            api_key,
            web_dir: None,
            ..Default::default()
        },
    )
}

async fn rpc(app: &axum::Router, payload: Value) -> (axum::http::StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("host", "localhost")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).to_string()))
    };
    (status, value)
}

fn initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": { "name": "recall-conformance", "version": "0.0.1" }
        }
    })
}

fn tool_call(id: u64, name: &str, args: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": args }
    })
}

fn unwrap_result(body: &Value) -> &Value {
    body.get("result")
        .unwrap_or_else(|| panic!("JSON-RPC result expected, got: {body}"))
}

fn tool_text(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(text).expect("tool output must be valid JSON")
}

#[tokio::test]
async fn conformance_initialize_tools_list_call_over_http() {
    let app = app_with(None);

    // 1) initialize — server identifies itself and its capabilities.
    let (status, body) = rpc(&app, initialize_request()).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let result = unwrap_result(&body);
    assert_eq!(result["serverInfo"]["name"], "cortex-mcp");
    // Wire pin: rmcp 3.3 answers an `initialize` naming `2026-07-28` with
    // `2025-11-25` — the newest version that still HAS an initialize
    // handshake, because the 2026-07-28 revision replaced the handshake with
    // per-request metadata (rmcp `negotiate_protocol_version`). Pinned so an
    // SDK bump that changes the wire answer is caught, not silently absorbed.
    assert_eq!(result["protocolVersion"], "2025-11-25");
    assert!(
        result["capabilities"]["tools"].is_object(),
        "tools capability must be advertised"
    );

    // 2) tools/list — exactly the six documented tools, each with an input schema.
    let (status, body) = rpc(
        &app,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let tools = unwrap_result(&body)["tools"]
        .as_array()
        .expect("tools array");
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "capture",
            "forget",
            "list_memories",
            "recall",
            "remember",
            "update_memory"
        ]
    );
    for tool in tools {
        assert!(
            tool["inputSchema"].is_object(),
            "every tool needs an input schema"
        );
        assert!(tool["description"].as_str().is_some());
    }

    // 3) tools/call remember -> memory stored and returned.
    let (status, body) = rpc(
        &app,
        tool_call(3, "remember", json!({"namespace": "conformance", "text": "the mcp transport speaks json-rpc", "tags": ["mcp"]})),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(unwrap_result(&body)["isError"], false);
    let stored = tool_text(unwrap_result(&body));
    let memory_id = stored["id"].as_str().expect("stored memory id").to_string();
    assert_eq!(stored["text"], "the mcp transport speaks json-rpc");

    // 4) tools/call recall -> ranked hit with breakdown.
    let (status, body) = rpc(
        &app,
        tool_call(
            4,
            "recall",
            json!({"namespace": "conformance", "query": "json-rpc transport", "k": 2}),
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let hits = tool_text(unwrap_result(&body));
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["id"], memory_id.as_str());
    assert!(hits[0]["breakdown"]["total"].as_f64().unwrap() > 0.0);

    // 4b) tools/call update_memory -> text revised, tags replaced.
    let (_, body) = rpc(
        &app,
        tool_call(
            10,
            "update_memory",
            json!({"id": memory_id, "text": "the transport now speaks json-rpc fluently", "tags": ["mcp", "updated"]}),
        ),
    )
    .await;
    let updated = tool_text(unwrap_result(&body));
    assert_eq!(
        updated["text"],
        "the transport now speaks json-rpc fluently"
    );
    assert_eq!(updated["tags"], json!(["mcp", "updated"]));
    assert_eq!(updated["id"], memory_id.as_str());

    // 4c) recall with a tag filter finds the updated memory; the wrong filter finds nothing.
    let (_, body) = rpc(
        &app,
        tool_call(
            11,
            "recall",
            json!({"namespace": "conformance", "query": "json-rpc", "tags": ["updated"]}),
        ),
    )
    .await;
    let hits = tool_text(unwrap_result(&body));
    assert_eq!(hits.as_array().unwrap().len(), 1);
    let (_, body) = rpc(
        &app,
        tool_call(
            12,
            "recall",
            json!({"namespace": "conformance", "query": "json-rpc", "tags": ["unrelated"]}),
        ),
    )
    .await;
    assert_eq!(tool_text(unwrap_result(&body)).as_array().unwrap().len(), 0);

    // 5) tools/call list_memories -> still present.
    let (_, body) = rpc(
        &app,
        tool_call(5, "list_memories", json!({"namespace": "conformance"})),
    )
    .await;
    let memories = tool_text(unwrap_result(&body));
    assert_eq!(memories.as_array().unwrap().len(), 1);

    // 6) tools/call forget -> deleted: true, then list is empty.
    let (_, body) = rpc(&app, tool_call(6, "forget", json!({"id": memory_id}))).await;
    let forgotten = tool_text(unwrap_result(&body));
    assert_eq!(forgotten["deleted"], true);

    let (_, body) = rpc(
        &app,
        tool_call(7, "list_memories", json!({"namespace": "conformance"})),
    )
    .await;
    assert_eq!(tool_text(unwrap_result(&body)).as_array().unwrap().len(), 0);

    // 6b) tools/call capture -> transcript auto-chunked into capture memories
    // (runs after the forget steps so their exact-count assertions stay intact).
    let long = "Decided to keep the storage layer boring. ".repeat(60); // ~2520 chars
    let (_, body) = rpc(
        &app,
        tool_call(
            13,
            "capture",
            json!({
                "namespace": "conformance",
                "transcript": [
                    {"role": "user", "content": "please summarize the storage decision", "at": 1_786_000_500},
                    {"role": "assistant", "content": long}
                ],
                "tags": ["session:storage"]
            }),
        ),
    )
    .await;
    assert_eq!(unwrap_result(&body)["isError"], false);
    let report = tool_text(unwrap_result(&body));
    assert_eq!(report["namespace"], "conformance");
    let captured = report["captured"].as_u64().expect("captured count");
    assert!(captured >= 3, "long message must be chunked: {report}");
    let memories = report["memories"].as_array().unwrap();
    assert_eq!(memories.len() as u64, captured);
    for memory in memories {
        assert_eq!(memory["source"], "capture");
        // The user message carries an explicit `at`; the assistant message
        // intentionally does not and takes the server clock.
        if memory["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "role:user")
        {
            assert_eq!(memory["created_at"], 1_786_000_500);
        }
        let tags: Vec<&str> = memory["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        assert!(tags.contains(&"session:storage"), "{tags:?}");
        assert!(tags.contains(&"role:user") || tags.contains(&"role:assistant"));
    }

    // Capture output is recallable through the same MCP surface.
    let (_, body) = rpc(
        &app,
        tool_call(
            14,
            "recall",
            json!({"namespace": "conformance", "query": "storage decision boring", "tags": ["session:storage"]}),
        ),
    )
    .await;
    let hits = tool_text(unwrap_result(&body));
    assert!(
        !hits.as_array().unwrap().is_empty(),
        "captured chunks must be recallable: {hits}"
    );

    // 7) unknown tool -> JSON-RPC protocol error (invalid params / tool missing).
    let (status, body) = rpc(&app, tool_call(8, "nonexistent", json!({}))).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body["error"].is_object(),
        "unknown tool must be a JSON-RPC error: {body}"
    );

    // 8) caller-fixable tool failure -> isError result with message.
    let (_, body) = rpc(
        &app,
        tool_call(
            9,
            "recall",
            json!({"namespace": "ghost-namespace", "query": "anything"}),
        ),
    )
    .await;
    let err = &body["error"];
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .contains("namespace not found"),
        "{body}"
    );
}

#[tokio::test]
async fn mcp_endpoint_honors_api_key() {
    let app = app_with(Some("mcp-secret".to_string()));

    // No key -> 401, no protocol data leaks.
    let (status, body) = rpc(&app, initialize_request()).await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().is_some());

    // Wrong key -> 401.
    let res = app
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json")
                .header("host", "localhost")
                .header("x-api-key", "nope")
                .body(Body::from(initialize_request().to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNAUTHORIZED);

    // Correct key -> full initialize succeeds.
    let res = app
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("host", "localhost")
                .header("authorization", "Bearer mcp-secret")
                .body(Body::from(initialize_request().to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(unwrap_result(&body)["serverInfo"]["name"], "cortex-mcp");
}
