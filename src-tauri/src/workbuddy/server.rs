//! Minimal embedded OpenAI-compatible HTTP kernel server (`127.0.0.1:<port>`).
//! Exposes strictly the 3 kernel endpoints:
//! - `GET  /health`
//! - `GET  /v1/models`
//! - `POST /v1/chat/completions` (streaming SSE passthrough + non-streaming aggregation)

use crate::workbuddy::protocol::{
    extract_conversation_id, prepare_chat_body, ChatMeta, UpstreamErrKind,
};
use crate::workbuddy::site::{account_is_global, new_message_id, unix_now_secs};
use crate::workbuddy::WorkBuddyKernel;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub struct RunningServer {
    pub port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl RunningServer {
    pub fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

pub async fn start_kernel_server(
    kernel: Arc<WorkBuddyKernel>,
    port: u16,
) -> Result<RunningServer, String> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("无法监听 {addr}（端口可能已被占用）: {e}"))?;

    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/v1/models", get(handle_models))
        .route("/models", get(handle_models))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(kernel);

    let (tx, rx) = oneshot::channel::<()>();

    tauri::async_runtime::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = rx.await;
        });
        let _ = server.await;
    });

    Ok(RunningServer {
        port,
        shutdown_tx: Some(tx),
    })
}

fn check_api_key(kernel: &WorkBuddyKernel, headers: &HeaderMap) -> Result<(), Response> {
    let expected = kernel.expected_api_key();
    if expected.is_empty() {
        return Ok(());
    }
    let auth_bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(str::trim);
    let x_api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);

    if auth_bearer == Some(expected.as_str()) || x_api_key == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(openai_error(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "无效的本地网关 API Key",
        ))
    }
}

fn openai_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "api_error",
                "code": code
            }
        })),
    )
        .into_response()
}

async fn handle_health(State(kernel): State<Arc<WorkBuddyKernel>>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "workbuddy2api-core",
        "loggedIn": kernel.is_logged_in()
    }))
}

async fn handle_models(
    State(kernel): State<Arc<WorkBuddyKernel>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = check_api_key(&kernel, &headers) {
        return resp;
    }

    match kernel.get_cached_or_fetch_models(false).await {
        Ok(models) => {
            let created = unix_now_secs();
            let data: Vec<Value> = models
                .into_iter()
                .map(|m| {
                    let mut obj = json!({
                        "id": m.id,
                        "object": "model",
                        "created": created,
                        "owned_by": if m.vendor.is_empty() { "workbuddy" } else { m.vendor.as_str() },
                        "name": m.name,
                    });
                    if m.vision {
                        obj["architecture"] = json!({
                            "input_modalities": ["text", "image"]
                        });
                    }
                    if m.tool_calling {
                        obj["supported_parameters"] = json!(["tools"]);
                    }
                    if m.context_window > 0 {
                        obj["context_length"] = json!(m.context_window);
                    }
                    if m.max_tokens > 0 {
                        obj["max_output_tokens"] = json!(m.max_tokens);
                    }
                    obj
                })
                .collect();
            Json(json!({
                "object": "list",
                "data": data
            }))
            .into_response()
        }
        Err(msg) => openai_error(StatusCode::SERVICE_UNAVAILABLE, "models_unavailable", &msg),
    }
}

async fn handle_chat_completions(
    State(kernel): State<Arc<WorkBuddyKernel>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(resp) = check_api_key(&kernel, &headers) {
        return resp;
    }

    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("请求体不是合法 JSON: {e}"),
            );
        }
    };
    let Some(obj) = parsed.as_object() else {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "请求体必须是 JSON 对象",
        );
    };

    let client_wants_stream = obj
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let raw_model = obj.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let conv_id = extract_conversation_id(obj);
    let conv_req_id = new_message_id();

    let mut acc = match kernel.ensure_fresh_account(false).await {
        Ok(a) => a,
        Err(msg) => {
            return openai_error(StatusCode::SERVICE_UNAVAILABLE, "no_account", &msg);
        }
    };

    let meta = ChatMeta {
        conversation_id: conv_id.clone(),
        cache_key_id: conv_id.clone(),
        conversation_request_id: conv_req_id.clone(),
        root_request_id: conv_req_id,
        trace_id: String::new(),
    };
    let prepared = prepare_chat_body(
        obj.clone(),
        account_is_global(&acc),
        &acc.uid,
        &conv_id,
    );

    let mut attempt_res = kernel.send_chat(&acc, &prepared, &meta).await;

    // On 401/SessionDead, force one token refresh and retry immediately
    if let Err(ref err) = attempt_res {
        if err.kind == UpstreamErrKind::SessionDead && !acc.refresh_token.is_empty() {
            if let Ok(refreshed) = kernel.ensure_fresh_account(true).await {
                acc = refreshed;
                attempt_res = kernel.send_chat(&acc, &prepared, &meta).await;
            }
        }
    }

    match attempt_res {
        Ok(resp) => {
            kernel.mark_chat_success();
            if client_wants_stream {
                passthrough_sse_response(resp)
            } else {
                aggregate_sse_to_json(resp, raw_model).await
            }
        }
        Err(err) => {
            kernel.record_chat_error(err.kind);
            let status = StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_GATEWAY);
            openai_error(status, err.kind.as_str(), &err.msg)
        }
    }
}

fn passthrough_sse_response(resp: reqwest::Response) -> Response {
    let stream = resp.bytes_stream().map(|item| {
        item.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    });
    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    response.headers_mut().insert(
        "x-accel-buffering",
        HeaderValue::from_static("no"),
    );
    response
}

#[derive(Default)]
struct AggregatedToolCall {
    id: String,
    name: String,
    arguments: String,
}

async fn aggregate_sse_to_json(resp: reqwest::Response, fallback_model: &str) -> Response {
    let mut stream = resp.bytes_stream();
    let mut line_buf = String::new();

    let mut id = format!("chatcmpl-{}", new_message_id());
    let mut model = fallback_model.to_string();
    let mut created = unix_now_secs();
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut finish_reason = String::from("stop");
    let mut usage: Option<Value> = None;
    let mut tool_calls: BTreeMap<usize, AggregatedToolCall> = BTreeMap::new();

    let mut process_line = |line: &str| {
        let trimmed = line.trim();
        let Some(data) = trimmed.strip_prefix("data:").map(str::trim) else {
            return;
        };
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(data) else {
            return;
        };
        if let Some(s) = parsed.get("id").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                id = s.to_string();
            }
        }
        if let Some(s) = parsed.get("model").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                model = s.to_string();
            }
        }
        if let Some(c) = parsed.get("created").and_then(|v| v.as_i64()) {
            created = c;
        }
        if let Some(u) = parsed.get("usage") {
            if !u.is_null() {
                usage = Some(u.clone());
            }
        }
        if let Some(choice) = parsed.get("choices").and_then(|c| c.get(0)) {
            if let Some(fr) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                if !fr.is_empty() {
                    finish_reason = fr.to_string();
                }
            }
            if let Some(delta) = choice.get("delta") {
                if let Some(c) = delta.get("content").and_then(|v| v.as_str()) {
                    content.push_str(c);
                }
                if let Some(r) = delta
                    .get("reasoning_content")
                    .or_else(|| delta.get("reasoning"))
                    .and_then(|v| v.as_str())
                {
                    reasoning.push_str(r);
                }
                if let Some(arr) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for (pos, tc) in arr.iter().enumerate() {
                        let idx = tc
                            .get("index")
                            .and_then(|v| v.as_u64())
                            .map(|n| n as usize)
                            .unwrap_or(pos);
                        let entry = tool_calls.entry(idx).or_default();
                        if let Some(tid) = tc.get("id").and_then(|v| v.as_str()) {
                            if !tid.is_empty() {
                                entry.id = tid.to_string();
                            }
                        }
                        if let Some(f) = tc.get("function") {
                            if let Some(n) = f.get("name").and_then(|v| v.as_str()) {
                                entry.name.push_str(n);
                            }
                            if let Some(a) = f.get("arguments").and_then(|v| v.as_str()) {
                                entry.arguments.push_str(a);
                            }
                        }
                    }
                }
            }
        }
    };

    while let Some(chunk_res) = stream.next().await {
        let Ok(bytes) = chunk_res else {
            break;
        };
        line_buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(nl) = line_buf.find('\n') {
            let line = line_buf[..nl].to_string();
            line_buf.drain(..=nl);
            process_line(&line);
        }
    }
    if !line_buf.trim().is_empty() {
        process_line(&line_buf);
    }

    let mut message = json!({
        "role": "assistant",
        "content": content
    });
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if !tool_calls.is_empty() {
        let tc_list: Vec<Value> = tool_calls
            .into_iter()
            .filter(|(_, t)| !t.name.is_empty())
            .enumerate()
            .map(|(i, (_, t))| {
                json!({
                    "id": if t.id.is_empty() { format!("call_{i}") } else { t.id },
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "arguments": t.arguments
                    }
                })
            })
            .collect();
        if !tc_list.is_empty() {
            message["tool_calls"] = Value::Array(tc_list);
        }
    }

    let mut payload = json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason
        }]
    });
    if let Some(u) = usage {
        payload["usage"] = u;
    }

    Json(payload).into_response()
}
