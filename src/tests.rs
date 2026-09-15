use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::opencode::{
    build_prompt_body, extract_text_parts, parse_server_url, OpencodeManager, OpencodeSettings,
    SseParser,
};

#[derive(Clone, Default)]
struct MockState {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    last_auth: Arc<Mutex<Option<String>>>,
}

fn test_manager() -> Arc<OpencodeManager> {
    OpencodeManager::new(OpencodeSettings {
        executable: "opencode".to_string(),
        directory: String::new(),
        agent: String::new(),
        model: String::new(),
        port: 0,
        extra_args: Vec::new(),
    })
}

async fn mock_opencode() -> (Arc<OpencodeManager>, MockState) {
    let state = MockState::default();

    let app =
        Router::new()
            .route(
                "/global/health",
                axum::routing::get(|headers: HeaderMap| async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let _ = auth;
                    Json(json!({ "healthy": true, "version": "test" }))
                }),
            )
            .route(
                "/session",
                axum::routing::get(
                    |State(state): State<MockState>, headers: HeaderMap| async move {
                        record(&state, &headers, "GET /session", json!(null)).await;
                        Json(json!([{ "id": "ses_list" }]))
                    },
                )
                .post(
                    |State(state): State<MockState>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        record(&state, &headers, "POST /session", body.clone()).await;
                        Json(json!({ "id": "ses_created", "title": body.get("title") }))
                    },
                ),
            )
            .route(
                "/session/{id}/prompt_async",
                post(
                    |State(state): State<MockState>,
                     headers: HeaderMap,
                     axum::extract::Path(id): axum::extract::Path<String>,
                     Json(body): Json<Value>| async move {
                        record(
                            &state,
                            &headers,
                            &format!("POST /session/{id}/prompt_async"),
                            body,
                        )
                        .await;
                        StatusCode::NO_CONTENT
                    },
                ),
            )
            .route(
                "/session/{id}/message",
                post(
                    |State(state): State<MockState>,
                     headers: HeaderMap,
                     axum::extract::Path(id): axum::extract::Path<String>,
                     Json(body): Json<Value>| async move {
                        record(
                            &state,
                            &headers,
                            &format!("POST /session/{id}/message"),
                            body,
                        )
                        .await;
                        Json(json!({
                            "info": { "role": "assistant" },
                            "parts": [
                                { "type": "text", "text": "hello from mock" },
                                { "type": "tool", "tool": "bash" },
                                { "type": "text", "text": "second line" }
                            ]
                        }))
                    },
                ),
            )
            .route(
                "/session/{id}/permissions/{pid}",
                post(
                    |State(state): State<MockState>,
                     headers: HeaderMap,
                     axum::extract::Path((id, pid)): axum::extract::Path<(String, String)>,
                     Json(body): Json<Value>| async move {
                        record(
                            &state,
                            &headers,
                            &format!("POST /session/{id}/permissions/{pid}"),
                            body,
                        )
                        .await;
                        Json(json!({ "ok": true }))
                    },
                ),
            )
            .route(
                "/permission/{id}/reply",
                post(
                    |State(state): State<MockState>,
                     headers: HeaderMap,
                     axum::extract::Path(id): axum::extract::Path<String>,
                     Json(body): Json<Value>| async move {
                        record(
                            &state,
                            &headers,
                            &format!("POST /permission/{id}/reply"),
                            body,
                        )
                        .await;
                        Json(json!({ "ok": true }))
                    },
                ),
            )
            .route(
                "/question",
                axum::routing::get(
                    |State(state): State<MockState>, headers: HeaderMap| async move {
                        record(&state, &headers, "GET /question", json!(null)).await;
                        Json(json!([]))
                    },
                ),
            )
            .route(
                "/question/{id}/reply",
                post(
                    |State(state): State<MockState>,
                     headers: HeaderMap,
                     axum::extract::Path(id): axum::extract::Path<String>,
                     Json(body): Json<Value>| async move {
                        record(
                            &state,
                            &headers,
                            &format!("POST /question/{id}/reply"),
                            body,
                        )
                        .await;
                        Json(json!({ "ok": true }))
                    },
                ),
            )
            .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("mock address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let manager = test_manager();
    manager
        .set_connection_for_test(format!("http://{addr}"), "test-password".to_string())
        .await;

    (manager, state)
}

async fn record(state: &MockState, headers: &HeaderMap, route: &str, body: Value) {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    *state.last_auth.lock().await = auth;
    state.requests.lock().await.push((route.to_string(), body));
}

#[tokio::test]
async fn adapter_lists_sessions_with_basic_auth() {
    let (manager, state) = mock_opencode().await;

    let sessions = manager.list_sessions().await.expect("list sessions");
    assert_eq!(sessions[0]["id"], "ses_list");

    let auth = state.last_auth.lock().await.clone().unwrap_or_default();
    assert!(
        auth.starts_with("Basic "),
        "expected basic auth, got {auth:?}"
    );
}

#[tokio::test]
async fn adapter_creates_session_with_title() {
    let (manager, state) = mock_opencode().await;

    let session = manager
        .create_session(Some("Voice call".to_string()))
        .await
        .expect("create session");
    assert_eq!(session["id"], "ses_created");

    let requests = state.requests.lock().await;
    let (_, body) = requests
        .iter()
        .find(|(route, _)| route == "POST /session")
        .expect("session request recorded");
    assert_eq!(body["title"], "Voice call");
}

#[tokio::test]
async fn adapter_prompt_async_sends_parts_and_agent() {
    let (manager, state) = mock_opencode().await;

    manager
        .prompt_async(
            "ses_1",
            "hello",
            Some("build"),
            Some("opencode/gpt-5.1-codex"),
        )
        .await
        .expect("prompt async");

    let requests = state.requests.lock().await;
    let (_, body) = requests
        .iter()
        .find(|(route, _)| route == "POST /session/ses_1/prompt_async")
        .expect("prompt recorded");
    assert_eq!(body["parts"][0]["type"], "text");
    assert_eq!(body["parts"][0]["text"], "hello");
    assert_eq!(body["agent"], "build");
    assert_eq!(body["model"]["providerID"], "opencode");
    assert_eq!(body["model"]["modelID"], "gpt-5.1-codex");
}

#[tokio::test]
async fn adapter_blocking_prompt_extracts_text_parts() {
    let (manager, _state) = mock_opencode().await;

    let text = manager
        .prompt_blocking("ses_1", "hi", None, None)
        .await
        .expect("blocking prompt");
    assert_eq!(text, "hello from mock\nsecond line");
}

#[tokio::test]
async fn adapter_replies_to_permissions_and_falls_back() {
    let (manager, state) = mock_opencode().await;

    manager
        .reply_permission("ses_1", "per_1", "once", false)
        .await
        .expect("permission reply");

    let requests = state.requests.lock().await;
    let (route, body) = requests
        .iter()
        .find(|(route, _)| route.contains("permissions/per_1"))
        .expect("permission request recorded");
    assert_eq!(route, "POST /session/ses_1/permissions/per_1");
    assert_eq!(body["response"], "once");
}

#[tokio::test]
async fn adapter_replies_to_questions() {
    let (manager, state) = mock_opencode().await;

    manager
        .reply_question("que_1", vec![vec!["Yes".to_string()]])
        .await
        .expect("question reply");

    let requests = state.requests.lock().await;
    let (_, body) = requests
        .iter()
        .find(|(route, _)| route == "POST /question/que_1/reply")
        .expect("question request recorded");
    assert_eq!(body["answers"][0][0], "Yes");
}

#[test]
fn parse_server_url_finds_managed_server_url() {
    let line = "opencode server listening on http://127.0.0.1:45123";
    assert_eq!(
        parse_server_url(line).as_deref(),
        Some("http://127.0.0.1:45123")
    );
    assert_eq!(
        parse_server_url("Server: https://localhost:4096 ready").as_deref(),
        Some("https://localhost:4096")
    );
    assert_eq!(parse_server_url("no url here"), None);
}

#[test]
fn sse_parser_handles_chunk_boundaries_and_multiline_data() {
    let mut parser = SseParser::new();
    assert!(parser.push("data: {\"type\":\"a\"}").is_empty());
    assert_eq!(parser.push("\n\n"), vec!["{\"type\":\"a\"}".to_string()]);

    assert!(parser.push("data: {\"type\":\"b\"}\n").is_empty());
    assert_eq!(
        parser.push("\ndata: part1\ndata: part2\n\n"),
        vec!["{\"type\":\"b\"}".to_string(), "part1\npart2".to_string()]
    );

    assert_eq!(
        parser.push("data: windows\r\n\r\n"),
        vec!["windows".to_string()]
    );
}

#[test]
fn prompt_body_resolves_model_and_agent() {
    let body = build_prompt_body(
        "run the tests",
        Some("build"),
        Some("opencode/gpt-5.1-codex"),
    );
    assert_eq!(body["parts"][0]["text"], "run the tests");
    assert_eq!(body["agent"], "build");
    assert_eq!(body["model"]["providerID"], "opencode");
    assert_eq!(body["model"]["modelID"], "gpt-5.1-codex");

    let body = build_prompt_body("plain", None, None);
    assert!(body.get("agent").is_none());
    assert!(body.get("model").is_none());
}

#[test]
fn extract_text_parts_skips_non_text_parts() {
    let response = json!({
        "parts": [
            { "type": "reasoning", "text": "thinking" },
            { "type": "text", "text": "one" },
            { "type": "tool", "tool": "bash" },
            { "type": "text", "text": "two" }
        ]
    });
    assert_eq!(extract_text_parts(&response), "one\ntwo");
}

#[test]
fn install_commands_are_whitelisted() {
    assert_eq!(
        crate::tools::install_command("official").as_deref(),
        Some("curl -fsSL https://opencode.ai/install | bash")
    );
    assert_eq!(
        crate::tools::install_command("npm").as_deref(),
        Some("npm install -g opencode-ai")
    );
    assert_eq!(crate::tools::install_command("rm -rf /"), None);
}

#[test]
fn install_methods_are_platform_specific() {
    let linux = crate::tools::opencode_install_methods("linux");
    assert!(linux.iter().any(|m| m.id == "official"));
    assert!(linux.iter().any(|m| m.id == "npm"));

    let windows = crate::tools::opencode_install_methods("windows");
    assert!(windows.iter().any(|m| m.id == "npm"));
    assert!(windows
        .iter()
        .all(|m| m.command != "sudo pacman -S opencode"));
}
