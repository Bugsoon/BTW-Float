use crate::config::AppConfig;
use crate::search::{self, SearchSource};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageInput {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub images: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStreamRequest {
    pub request_id: String,
    pub messages: Vec<ChatMessageInput>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    #[serde(default)]
    pub web_search: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatDeltaPayload {
    request_id: String,
    delta: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatStatusPayload {
    request_id: String,
    status: String,
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatSourcesPayload {
    request_id: String,
    sources: Vec<SearchSource>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatDonePayload {
    request_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatErrorPayload {
    request_id: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub label: String,
    pub vision: bool,
    pub tool_calling: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchModelsResponse {
    pub normalized_base_url: String,
    pub models: Vec<DiscoveredModel>,
}

fn normalize_input_base_url(raw: &str) -> String {
    let mut s = raw.trim().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/completions", "/models"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.trim_end_matches('/').to_string();
        }
    }
    s
}

fn is_non_chat_model(id_lower: &str) -> bool {
    let skip_keywords = [
        "embedding",
        "embed-",
        "bge-",
        "rerank",
        "whisper",
        "tts-",
        "dall-e",
        "moderation",
        "stable-diffusion",
        "flux.",
        "flux-",
        "kokoro",
        "fish-speech",
        "cosyvoice",
        "sensevoice",
    ];
    skip_keywords.iter().any(|kw| id_lower.contains(kw))
}

fn infer_vision_support(id_lower: &str, item: &Value) -> bool {
    // 1. Check OpenRouter / rich metadata `architecture.input_modalities`
    if let Some(mods) = item
        .get("architecture")
        .and_then(|a| a.get("input_modalities"))
        .and_then(|m| m.as_array())
    {
        if mods.iter().any(|v| v.as_str() == Some("image")) {
            return true;
        }
    }

    // 2. Common multimodal model ID patterns
    let vision_patterns = [
        "gpt-4o",
        "gpt-4.1",
        "gpt-4.5",
        "gpt-5",
        "chatgpt-4o",
        "o1",
        "o3",
        "o4",
        "gemini-",
        "claude-3",
        "claude-sonnet-4",
        "claude-opus-4",
        "claude-haiku-4",
        "-vl",
        "vl-",
        "/vl",
        "vision",
        "pixtral",
        "llava",
        "minicpm-v",
        "internvl",
        "glm-4v",
        "glm-4.1v",
        "qvq",
        "step-1v",
        "kimi-latest",
        "kimi-k2.5",
        "grok-2-vision",
        "grok-4",
        "gemma-3-",
        "llama-3.2-11b-vision",
        "llama-3.2-90b-vision",
        "llama-4",
    ];
    vision_patterns.iter().any(|p| id_lower.contains(p))
}

fn infer_tool_calling(id_lower: &str, item: &Value) -> bool {
    if let Some(params) = item.get("supported_parameters").and_then(|p| p.as_array()) {
        return params.iter().any(|v| v.as_str() == Some("tools"));
    }
    // Pure reasoning-only early preview models without tool support
    !(id_lower.contains("o1-mini")
        || id_lower.contains("o1-preview")
        || id_lower.contains("qvq"))
}

fn short_display_label(id: &str, raw_name: Option<&str>) -> String {
    if let Some(name) = raw_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() && trimmed.len() <= 36 {
            return trimmed.to_string();
        }
    }
    id.rsplit('/').next().unwrap_or(id).to_string()
}

fn parse_models_payload(val: &Value) -> Vec<DiscoveredModel> {
    let arr_opt = val
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| val.get("models").and_then(|m| m.as_array()))
        .or_else(|| val.as_array());

    let Some(arr) = arr_opt else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for item in arr {
        let raw_id = item
            .get("id")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if raw_id.is_empty() {
            continue;
        }
        // Gemini OpenAI compat sometimes prefixes "models/"
        let clean_id = raw_id.strip_prefix("models/").unwrap_or(raw_id).to_string();
        let lower = clean_id.to_ascii_lowercase();
        if is_non_chat_model(&lower) {
            continue;
        }
        if out.iter().any(|m: &DiscoveredModel| m.id == clean_id) {
            continue;
        }

        let raw_name = item.get("name").and_then(|v| v.as_str());
        let label = short_display_label(&clean_id, if raw_name == Some(raw_id) { None } else { raw_name });
        let vision = infer_vision_support(&lower, item);
        let tool_calling = infer_tool_calling(&lower, item);

        out.push(DiscoveredModel {
            id: clean_id,
            label,
            vision,
            tool_calling,
        });
    }

    out
}

/// Queries `{base_url}/models` (with automatic `/v1/models` and Ollama `/api/tags` fallback)
/// to discover available models and auto-correct missing `/v1` suffixes.
pub async fn fetch_provider_models(
    base_url: &str,
    api_key: &str,
) -> Result<FetchModelsResponse, String> {
    let clean_base = normalize_input_base_url(base_url);
    if clean_base.is_empty() {
        return Err("请先填写 Base URL".to_string());
    }
    if !clean_base.starts_with("http://") && !clean_base.starts_with("https://") {
        return Err("Base URL 需以 http:// 或 https:// 开头".to_string());
    }

    let client = search::build_http_client(15)?;
    let key = api_key.trim();

    let mut candidates: Vec<(String, String)> = vec![(
        format!("{clean_base}/models"),
        clean_base.clone(),
    )];

    if !clean_base.ends_with("/v1") && !clean_base.contains("/v1beta") {
        let with_v1 = format!("{clean_base}/v1");
        candidates.push((format!("{with_v1}/models"), with_v1.clone()));
        if clean_base.contains("11434") || clean_base.contains("localhost") || clean_base.contains("127.0.0.1") {
            candidates.push((format!("{clean_base}/api/tags"), with_v1));
        }
    }

    let mut last_err = String::from("未能获取到模型列表");

    for (endpoint, resolved_base) in candidates {
        let mut req = client.get(&endpoint).header("Accept", "application/json");
        if !key.is_empty() {
            req = req.bearer_auth(key);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    if status.as_u16() == 401 || status.as_u16() == 403 {
                        return Err(format!("认证失败 (HTTP {status})，请检查 API Key 是否正确"));
                    }
                    last_err = format!("接口 {endpoint} 返回 HTTP {status}");
                    continue;
                }
                match resp.json::<Value>().await {
                    Ok(json_val) => {
                        let models = parse_models_payload(&json_val);
                        if !models.is_empty() {
                            return Ok(FetchModelsResponse {
                                normalized_base_url: resolved_base,
                                models,
                            });
                        }
                        last_err = "接口返回成功，但未解析到可用对话模型".to_string();
                    }
                    Err(e) => {
                        last_err = format!("解析模型列表 JSON 失败: {e}");
                    }
                }
            }
            Err(e) => {
                last_err = format!("连接失败: {e}");
            }
        }
    }

    Err(last_err)
}

#[derive(Default)]
pub struct StreamRegistry {
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl StreamRegistry {
    pub fn insert(&self, id: String, handle: JoinHandle<()>) {
        if let Some(old) = self.tasks.lock().insert(id, handle) {
            old.abort();
        }
    }

    pub fn abort(&self, id: &str) {
        if let Some(handle) = self.tasks.lock().remove(id) {
            handle.abort();
        }
    }

    pub fn remove(&self, id: &str) {
        self.tasks.lock().remove(id);
    }

    pub fn abort_all(&self) {
        let mut guard = self.tasks.lock();
        for (_, h) in guard.drain() {
            h.abort();
        }
    }
}

const HIDDEN_BLOCKS: &[(&str, &str)] = &[
    ("<think>", "</think>"),
    ("<thought>", "</thought>"),
    ("<tool_call>", "</tool_call>"),
    ("<｜tool▁calls▁begin｜>", "<｜tool▁calls▁end｜>"),
];

/// Real-time streaming filter that strips `<think>...</think>`, `<thought>...</thought>`,
/// and leaked raw tool-call blocks across arbitrary SSE chunk boundaries,
/// while suppressing leading whitespace so only clean final output reaches the UI.
struct ThinkFilter {
    active_close_tag: Option<&'static str>,
    pending: String,
    trim_leading: bool,
}

impl ThinkFilter {
    fn new() -> Self {
        Self {
            active_close_tag: None,
            pending: String::new(),
            trim_leading: true,
        }
    }

    fn push(&mut self, chunk: &str) -> (String, bool) {
        self.pending.push_str(chunk);
        let mut out = String::new();
        let mut saw_hidden = false;

        loop {
            if let Some(close_tag) = self.active_close_tag {
                saw_hidden = true;
                if let Some(end_idx) = self.pending.find(close_tag) {
                    self.pending.drain(..end_idx + close_tag.len());
                    self.active_close_tag = None;
                    self.trim_leading = true;
                    continue;
                }
                let keep = longest_suffix_prefix(&self.pending, close_tag);
                let drain_len = self.pending.len().saturating_sub(keep);
                if drain_len > 0 {
                    self.pending.drain(..drain_len);
                }
                break;
            } else {
                // Find earliest opening hidden tag in pending buffer
                let mut earliest: Option<(usize, &'static str, &'static str)> = None;
                for &(open_tag, close_tag) in HIDDEN_BLOCKS {
                    if let Some(idx) = self.pending.find(open_tag) {
                        if earliest.is_none_or(|(best_idx, _, _)| idx < best_idx) {
                            earliest = Some((idx, open_tag, close_tag));
                        }
                    }
                }

                if let Some((start_idx, open_tag, close_tag)) = earliest {
                    let before = self.pending[..start_idx].to_string();
                    self.push_visible(&mut out, &before);
                    self.pending.drain(..start_idx + open_tag.len());
                    self.active_close_tag = Some(close_tag);
                    saw_hidden = true;
                    continue;
                }

                let keep = HIDDEN_BLOCKS
                    .iter()
                    .map(|&(open_tag, _)| longest_suffix_prefix(&self.pending, open_tag))
                    .max()
                    .unwrap_or(0);
                let emit_len = self.pending.len().saturating_sub(keep);
                if emit_len > 0 {
                    let slice = self.pending[..emit_len].to_string();
                    self.push_visible(&mut out, &slice);
                    self.pending.drain(..emit_len);
                }
                break;
            }
        }

        (out, saw_hidden)
    }

    fn flush(&mut self) -> String {
        if self.active_close_tag.is_some() {
            self.pending.clear();
            return String::new();
        }
        let mut out = String::new();
        let rem = std::mem::take(&mut self.pending);
        self.push_visible(&mut out, &rem);
        out
    }

    fn push_visible(&mut self, out: &mut String, s: &str) {
        if self.trim_leading {
            let trimmed = s.trim_start_matches(['\n', '\r', ' ', '\t']);
            if !trimmed.is_empty() {
                self.trim_leading = false;
                out.push_str(trimmed);
            }
        } else {
            out.push_str(s);
        }
    }
}

fn longest_suffix_prefix(haystack: &str, needle: &str) -> usize {
    let max_check = haystack.len().min(needle.len().saturating_sub(1));
    for len in (1..=max_check).rev() {
        if needle.is_char_boundary(len)
            && haystack.is_char_boundary(haystack.len() - len)
            && haystack.ends_with(&needle[..len])
        {
            return len;
        }
    }
    0
}

#[derive(Default, Clone)]
struct AccumulatedToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolMode {
    Auto,
    ForceNone,
    Disabled,
}

enum RoundOutcome {
    Finished {
        visible_text: String,
    },
    ToolCalls {
        calls: Vec<AccumulatedToolCall>,
        visible_text: String,
        reasoning_text: String,
    },
}

pub fn spawn_chat_stream(
    app: AppHandle,
    cfg: AppConfig,
    req: ChatStreamRequest,
    registry: Arc<StreamRegistry>,
) -> Result<(), String> {
    let provider = req
        .provider_id
        .as_deref()
        .and_then(|pid| cfg.providers.iter().find(|p| p.id == pid))
        .or_else(|| cfg.providers.iter().find(|p| p.id == cfg.active_provider_id))
        .or_else(|| cfg.providers.first())
        .cloned()
        .ok_or_else(|| "尚未配置模型供应商，请先打开设置配置 API".to_string())?;

    let model_id = req
        .model_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.active_model_id.clone());

    let supports_tools = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.tool_calling)
        .unwrap_or(true);

    if provider.base_url.trim().is_empty() {
        return Err("API Base URL 不能为空，请在设置中配置".to_string());
    }
    if model_id.trim().is_empty() {
        return Err("模型 ID 不能为空，请在设置中配置".to_string());
    }

    let request_id = req.request_id.clone();
    let req_id_for_reg = req.request_id.clone();
    let reg_for_task = registry.clone();
    let req_id_cleanup = request_id.clone();

    let handle = tauri::async_runtime::spawn(async move {
        let res = run_agent_chat_flow(
            &app,
            &provider.base_url,
            &provider.api_key,
            &model_id,
            supports_tools,
            &req,
        )
        .await;
        match res {
            Ok(()) => {
                let _ = app.emit(
                    "chat:done",
                    ChatDonePayload {
                        request_id: request_id.clone(),
                    },
                );
            }
            Err(err_msg) => {
                let _ = app.emit(
                    "chat:error",
                    ChatErrorPayload {
                        request_id: request_id.clone(),
                        message: err_msg,
                    },
                );
            }
        }
        reg_for_task.remove(&req_id_cleanup);
    });

    registry.insert(req_id_for_reg, handle);
    Ok(())
}

fn agent_tools_schema() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the live web for up-to-date facts, news, documentation, error messages, or current events. Formulate concise, targeted keywords.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Concise search query keywords"
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "fetch_url",
                "description": "Read the main text content of a specific http/https webpage URL when search snippets need deeper inspection.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Full http/https URL to read"
                        }
                    },
                    "required": ["url"]
                }
            }
        }
    ])
}

fn build_base_messages(sys_prompt: &str, messages: &[ChatMessageInput]) -> Vec<Value> {
    let mut messages_json = vec![json!({
        "role": "system",
        "content": sys_prompt
    })];

    for m in messages {
        if m.images.is_empty() {
            messages_json.push(json!({
                "role": m.role,
                "content": m.content
            }));
        } else {
            let mut parts = Vec::new();
            if !m.content.trim().is_empty() {
                parts.push(json!({ "type": "text", "text": m.content }));
            }
            for img in &m.images {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": { "url": img }
                }));
            }
            messages_json.push(json!({
                "role": m.role,
                "content": parts
            }));
        }
    }

    messages_json
}

async fn run_agent_chat_flow(
    app: &AppHandle,
    base_url: &str,
    api_key: &str,
    model_id: &str,
    supports_tools: bool,
    req: &ChatStreamRequest,
) -> Result<(), String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let client = search::build_http_client(45)?;

    let mut collected_sources: Vec<SearchSource> = Vec::new();
    let mut tool_notes: Vec<String> = Vec::new();
    let base_sys_prompt = "你是 BTW-Float，一位极简、高效的 Windows 桌面随手问答助手。请直接给出清晰、准确、精炼的最终中文回答，避免冗长寒暄；涉及代码或步骤时结构化呈现。";
    let mut sys_prompt = base_sys_prompt.to_string();

    if req.web_search && supports_tools {
        sys_prompt.push_str("\n当问题涉及实时信息、陌生概念/模型、不确定事实或具体网址时，请优先在 1 轮内调用 `web_search`（必要时调用 `fetch_url` 阅读网页正文），拿到结果后立即综合信息完整回答用户问题，不要反复无意义搜索，也绝不可不输出回答正文。");
    }

    // Fallback for models with toolCalling disabled: perform a single pre-search when web_search is enabled
    if req.web_search && !supports_tools {
        if let Some(last_user) = req.messages.iter().rev().find(|m| m.role == "user") {
            let query: String = last_user.content.chars().take(120).collect();
            let _ = app.emit(
                "chat:status",
                ChatStatusPayload {
                    request_id: req.request_id.clone(),
                    status: "searching".to_string(),
                    detail: Some(format!("正在联网检索「{}」...", truncate_label(&query, 26))),
                },
            );
            if let Ok(found) = search::free_web_search(&query, 5).await {
                if !found.is_empty() {
                    let mut ctx = String::from("\n\n【实时联网参考资料】:\n");
                    for (idx, s) in found.iter().enumerate() {
                        ctx.push_str(&format!(
                            "[{}] {} ({})\n摘要: {}\n\n",
                            idx + 1,
                            s.title,
                            s.url,
                            s.snippet
                        ));
                    }
                    sys_prompt.push_str(&ctx);
                    collected_sources = found;
                    let _ = app.emit(
                        "chat:sources",
                        ChatSourcesPayload {
                            request_id: req.request_id.clone(),
                            sources: collected_sources.clone(),
                        },
                    );
                }
            }
        }
    }

    let mut messages_json = build_base_messages(&sys_prompt, &req.messages);
    let max_rounds = if req.web_search && supports_tools { 3 } else { 1 };
    let mut total_visible = String::new();

    for round in 0..max_rounds {
        let tool_mode = if !req.web_search || !supports_tools {
            ToolMode::Disabled
        } else if round + 1 < max_rounds {
            ToolMode::Auto
        } else {
            ToolMode::ForceNone
        };

        if round == 0 {
            let _ = app.emit(
                "chat:status",
                ChatStatusPayload {
                    request_id: req.request_id.clone(),
                    status: "connecting".to_string(),
                    detail: None,
                },
            );
        }

        let round_res = execute_single_sse_round(
            app,
            &client,
            &url,
            api_key,
            model_id,
            &req.request_id,
            &messages_json,
            tool_mode,
        )
        .await;

        let outcome = match round_res {
            Ok(o) => o,
            Err(e) => {
                // If a third-party gateway choked on multi-round `role: "tool"` messages (especially with images),
                // fall through to our clean tool-free synthesis below if we already gathered search results!
                if round > 0 && !tool_notes.is_empty() {
                    break;
                }
                return Err(e);
            }
        };

        match outcome {
            RoundOutcome::Finished { visible_text } => {
                total_visible.push_str(&visible_text);
                break;
            }
            RoundOutcome::ToolCalls {
                calls,
                visible_text,
                reasoning_text,
            } => {
                total_visible.push_str(&visible_text);
                // If gateway ignored `tool_choice: "none"` on the final round, exit loop and synthesize cleanly
                if tool_mode != ToolMode::Auto {
                    break;
                }

                // Record assistant tool_calls message (preserve empty string content & reasoning_content for DeepSeek/OpenAI-compat gateways)
                let tc_json: Vec<Value> = calls
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let id = if c.id.is_empty() {
                            format!("call_{round}_{i}")
                        } else {
                            c.id.clone()
                        };
                        json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": c.arguments
                            }
                        })
                    })
                    .collect();

                let mut assistant_msg = json!({
                    "role": "assistant",
                    "content": visible_text,
                    "tool_calls": tc_json
                });
                if !reasoning_text.is_empty() {
                    assistant_msg["reasoning_content"] = json!(reasoning_text);
                }
                messages_json.push(assistant_msg);

                // Execute each tool call and append tool result messages
                for (i, call) in calls.into_iter().enumerate() {
                    let call_id = if call.id.is_empty() {
                        format!("call_{round}_{i}")
                    } else {
                        call.id
                    };
                    let args: Value =
                        serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));

                    let tool_output = match call.name.as_str() {
                        "web_search" => {
                            let query = args
                                .get("query")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let _ = app.emit(
                                "chat:status",
                                ChatStatusPayload {
                                    request_id: req.request_id.clone(),
                                    status: "searching".to_string(),
                                    detail: Some(format!(
                                        "正在搜索「{}」...",
                                        truncate_label(&query, 28)
                                    )),
                                },
                            );
                            match search::free_web_search(&query, 5).await {
                                Ok(results) => {
                                    if !results.is_empty() {
                                        let mut note = format!("检索词「{query}」结果:\n");
                                        for (idx, item) in results.iter().enumerate() {
                                            note.push_str(&format!(
                                                "- [{}] {}: {}\n",
                                                idx + 1,
                                                item.title,
                                                item.snippet
                                            ));
                                            if !collected_sources.iter().any(|s| s.url == item.url) {
                                                collected_sources.push(item.clone());
                                            }
                                        }
                                        tool_notes.push(note);
                                    }
                                    if !collected_sources.is_empty() {
                                        let _ = app.emit(
                                            "chat:sources",
                                            ChatSourcesPayload {
                                                request_id: req.request_id.clone(),
                                                sources: collected_sources.clone(),
                                            },
                                        );
                                    }
                                    serde_json::to_string(&results).unwrap_or_else(|_| "[]".into())
                                }
                                Err(e) => json!({ "error": e }).to_string(),
                            }
                        }
                        "fetch_url" => {
                            let target_url = args
                                .get("url")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let _ = app.emit(
                                "chat:status",
                                ChatStatusPayload {
                                    request_id: req.request_id.clone(),
                                    status: "searching".to_string(),
                                    detail: Some(format!(
                                        "正在阅读网页 {}...",
                                        truncate_label(&target_url, 30)
                                    )),
                                },
                            );
                            match search::fetch_webpage_text(&target_url, 4800).await {
                                Ok(content) => {
                                    if !content.trim().is_empty() {
                                        let excerpt: String = content.chars().take(1600).collect();
                                        tool_notes.push(format!("网页正文 ({target_url}):\n{excerpt}"));
                                    }
                                    json!({
                                        "url": target_url,
                                        "content": content
                                    })
                                    .to_string()
                                }
                                Err(e) => json!({ "error": e }).to_string(),
                            }
                        }
                        other => json!({ "error": format!("Unknown tool: {other}") }).to_string(),
                    };

                    messages_json.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": tool_output
                    }));
                }

                let _ = app.emit(
                    "chat:status",
                    ChatStatusPayload {
                        request_id: req.request_id.clone(),
                        status: "thinking".to_string(),
                        detail: Some("正在整合搜索结果...".to_string()),
                    },
                );
            }
        }
    }

    // Bulletproof Clean Synthesis Fallback:
    // If the model searched the web (`!tool_notes.is_empty()`) yet emitted ZERO visible answer text
    // (common on OpenAI-compat relays when combining Vision `image_url` + multi-turn `role: "tool"` or `tool_choice: "none"`),
    // make one clean tool-free streaming completion with the gathered findings injected as reference text!
    if total_visible.trim().is_empty() && !tool_notes.is_empty() {
        let _ = app.emit(
            "chat:status",
            ChatStatusPayload {
                request_id: req.request_id.clone(),
                status: "thinking".to_string(),
                detail: Some("正在汇总最终答案...".to_string()),
            },
        );
        let fallback_sys = format!(
            "{base_sys_prompt}\n\n【已检索到的实时参考资料（请直接结合用户提问与图片给出准确、清晰的中文回答）】:\n{}",
            tool_notes.join("\n\n")
        );
        let clean_messages = build_base_messages(&fallback_sys, &req.messages);
        let _ = execute_single_sse_round(
            app,
            &client,
            &url,
            api_key,
            model_id,
            &req.request_id,
            &clean_messages,
            ToolMode::Disabled,
        )
        .await?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_single_sse_round(
    app: &AppHandle,
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    model_id: &str,
    request_id: &str,
    messages_json: &[Value],
    tool_mode: ToolMode,
) -> Result<RoundOutcome, String> {
    let mut body = json!({
        "model": model_id,
        "messages": messages_json,
        "stream": true
    });

    match tool_mode {
        ToolMode::Auto => {
            body["tools"] = agent_tools_schema();
            body["tool_choice"] = json!("auto");
        }
        ToolMode::ForceNone => {
            body["tools"] = agent_tools_schema();
            body["tool_choice"] = json!("none");
        }
        ToolMode::Disabled => {}
    }

    let mut req_builder = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream");

    if !api_key.trim().is_empty() {
        req_builder = req_builder.bearer_auth(api_key.trim());
    }

    let response = req_builder
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        // If model/gateway rejected `tools` or `tool_choice` parameter (HTTP 400), gracefully retry without tools!
        if tool_mode != ToolMode::Disabled && status.as_u16() == 400 {
            return Box::pin(execute_single_sse_round(
                app,
                client,
                url,
                api_key,
                model_id,
                request_id,
                messages_json,
                ToolMode::Disabled,
            ))
            .await;
        }
        let snippet: String = text.chars().take(260).collect();
        return Err(format!("接口返回 HTTP {status}: {snippet}"));
    }

    let mut stream = response.bytes_stream();
    let mut line_buf = String::new();
    let mut raw_non_sse = String::new();
    let mut saw_sse_data = false;
    let mut filter = ThinkFilter::new();
    let mut accumulated_visible = String::new();
    let mut accumulated_reasoning = String::new();
    let mut emitted_thinking = false;
    let mut tool_map: BTreeMap<usize, AccumulatedToolCall> = BTreeMap::new();

    let mut process_json_chunk = |parsed: &Value| -> Result<(), String> {
        if let Some(err_obj) = parsed.get("error") {
            let msg = err_obj
                .get("message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| err_obj.to_string());
            return Err(format!("模型接口返回错误: {msg}"));
        }

        let Some(choice) = parsed.get("choices").and_then(|c| c.get(0)) else {
            return Ok(());
        };
        let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) else {
            return Ok(());
        };

        // Accumulate OpenAI tool_calls (streaming `delta.tool_calls` or buffered `message.tool_calls`)
        if let Some(tc_arr) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for (pos, tc) in tc_arr.iter().enumerate() {
                let idx = tc
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(pos);
                let entry = tool_map.entry(idx).or_default();
                if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                    if !id.is_empty() {
                        entry.id = id.to_string();
                    }
                }
                if let Some(func) = tc.get("function") {
                    if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                        entry.name.push_str(name);
                    }
                    if let Some(args_chunk) = func.get("arguments").and_then(|v| v.as_str()) {
                        entry.arguments.push_str(args_chunk);
                    }
                }
            }
        }

        // Accumulate reasoning_content / reasoning (hidden from UI, preserved for tool-call rounds)
        if let Some(r_str) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(|v| v.as_str())
        {
            if !r_str.is_empty() {
                accumulated_reasoning.push_str(r_str);
                if accumulated_visible.is_empty() && !emitted_thinking {
                    emitted_thinking = true;
                    let _ = app.emit(
                        "chat:status",
                        ChatStatusPayload {
                            request_id: request_id.to_string(),
                            status: "thinking".to_string(),
                            detail: None,
                        },
                    );
                }
            }
        }

        if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                let (visible, saw_hidden_tag) = filter.push(content);
                if saw_hidden_tag && accumulated_visible.is_empty() && !emitted_thinking {
                    emitted_thinking = true;
                    let _ = app.emit(
                        "chat:status",
                        ChatStatusPayload {
                            request_id: request_id.to_string(),
                            status: "thinking".to_string(),
                            detail: None,
                        },
                    );
                }
                if !visible.is_empty() {
                    accumulated_visible.push_str(&visible);
                    let _ = app.emit(
                        "chat:delta",
                        ChatDeltaPayload {
                            request_id: request_id.to_string(),
                            delta: visible,
                        },
                    );
                }
            }
        }

        Ok(())
    };

    while let Some(chunk_res) = stream.next().await {
        let bytes = chunk_res.map_err(|e| format!("读取流失败: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        line_buf.push_str(&text);

        while let Some(nl) = line_buf.find('\n') {
            let line = line_buf[..nl].trim().to_string();
            line_buf.drain(..=nl);

            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(data) = line.strip_prefix("data:").map(|s| s.trim()) {
                saw_sse_data = true;
                if data == "[DONE]" {
                    break;
                }
                if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                    process_json_chunk(&parsed)?;
                }
            } else if !saw_sse_data && raw_non_sse.len() < 262_144 {
                raw_non_sse.push_str(&line);
            }
        }
    }

    // Handle trailing partial line or buffered plain-JSON response from non-SSE gateways
    let rem_line = line_buf.trim();
    if !rem_line.is_empty() {
        if let Some(data) = rem_line.strip_prefix("data:").map(|s| s.trim()) {
            saw_sse_data = true;
            if data != "[DONE]" {
                if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                    process_json_chunk(&parsed)?;
                }
            }
        } else if !saw_sse_data {
            raw_non_sse.push_str(rem_line);
        }
    }

    if !saw_sse_data && !raw_non_sse.trim().is_empty() {
        if let Ok(parsed) = serde_json::from_str::<Value>(raw_non_sse.trim()) {
            process_json_chunk(&parsed)?;
        }
    }

    drop(process_json_chunk);

    let tail = filter.flush();
    if !tail.is_empty() {
        accumulated_visible.push_str(&tail);
        let _ = app.emit(
            "chat:delta",
            ChatDeltaPayload {
                request_id: request_id.to_string(),
                delta: tail,
            },
        );
    }

    let valid_calls: Vec<AccumulatedToolCall> = tool_map
        .into_values()
        .filter(|t| !t.name.is_empty())
        .collect();

    if !valid_calls.is_empty() {
        return Ok(RoundOutcome::ToolCalls {
            calls: valid_calls,
            visible_text: accumulated_visible,
            reasoning_text: accumulated_reasoning,
        });
    }

    Ok(RoundOutcome::Finished {
        visible_text: accumulated_visible,
    })
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max_chars).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_input_base_url, parse_models_payload, ThinkFilter};
    use serde_json::json;

    #[test]
    fn normalizes_url_and_parses_discovered_models() {
        assert_eq!(
            normalize_input_base_url("https://api.deepseek.com/v1/chat/completions/"),
            "https://api.deepseek.com/v1"
        );
        let sample = json!({
            "data": [
                { "id": "deepseek-chat" },
                { "id": "text-embedding-3-small" },
                { "id": "google/gemini-2.5-flash", "name": "Gemini 2.5 Flash" }
            ]
        });
        let models = parse_models_payload(&sample);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "deepseek-chat");
        assert!(!models[0].vision);
        assert_eq!(models[1].id, "google/gemini-2.5-flash");
        assert!(models[1].vision);
    }

    #[test]
    fn strips_split_think_and_tool_tags_and_leading_whitespace() {
        let mut f = ThinkFilter::new();
        let chunks = [
            "\n\n  <thi",
            "nk>internal reasoning...",
            "</th",
            "ink>\n\n<tool_ca",
            "ll>{\"name\":\"web_search\"}</tool_call>\nFinal ",
            "answer!",
        ];
        let mut out = String::new();
        let mut saw = false;
        for c in chunks {
            let (vis, s) = f.push(c);
            out.push_str(&vis);
            saw |= s;
        }
        out.push_str(&f.flush());
        assert!(saw);
        assert_eq!(out, "Final answer!");

        // Pure whitespace must never emit blank delta
        let mut f_ws = ThinkFilter::new();
        let (vis_ws, _) = f_ws.push("  \n\n\t ");
        assert_eq!(format!("{vis_ws}{}", f_ws.flush()), "");
    }

    #[test]
    fn passes_plain_text_through() {
        let mut f = ThinkFilter::new();
        let (a, s1) = f.push("Hello <world>");
        let b = f.flush();
        assert!(!s1);
        assert_eq!(format!("{a}{b}"), "Hello <world>");
    }
}
