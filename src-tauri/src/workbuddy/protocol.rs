//! Core WorkBuddy2API wire protocol: real-device outbound headers, chat payload normalization,
//! gzip compression, and upstream error classification (ported from misakano7545/workbuddy2api-panel).

use crate::config::WorkBuddyAccount;
use crate::workbuddy::site::{
    account_is_global, build_cache_key_v4, build_time_ordered_uuid, chat_base_for, chat_ua_for,
    client_version_for, host_of, new_message_id, new_span_id, new_time_ordered_uuid_v7,
    new_trace_id, rpc_ua_for, v3_probe_ua_for, CLIENT_IDENTITY_PLATFORM, CLIENT_IDENTITY_PRODUCT,
    REALM_CN, REALM_GLOBAL,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flate2::{write::GzEncoder, Compression};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::io::Write;

const CLIENT_ARCH: &str = "x64";
const CLIENT_OS: &str = "Windows";
const CLIENT_LANG: &str = "js";
const CLIENT_RUNTIME: &str = "node";
const CLIENT_NODE_VER: &str = "v22.21.1";
const CLIENT_SDK_VER: &str = "6.25.0";
const CONSOLE_SYSTEM_FALLBACK: &str =
    "Follow the user's instructions and respond in the user's language.";
const GZIP_MIN_CHAT_BODY: usize = 2048;
const GPT_MIN_MAX_TOKENS: i64 = 16;

#[derive(Debug, Clone, Default)]
pub struct ChatMeta {
    pub conversation_id: String,
    pub cache_key_id: String,
    pub conversation_request_id: String,
    pub root_request_id: String,
    pub trace_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamErrKind {
    HardCredit,
    SoftRate,
    SessionDead,
    NotFound,
    Server,
    ContentBlocked,
    BadParams,
    AccountFault,
    ChannelRefused,
    ModelBlocked,
    WafBlock,
    PromptTooLong,
    ImageInvalid,
    UpstreamBusy,
    Client,
}

impl UpstreamErrKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HardCredit => "hard_credit",
            Self::SoftRate => "soft_rate",
            Self::SessionDead => "session_dead",
            Self::NotFound => "not_found",
            Self::Server => "server",
            Self::ContentBlocked => "content_blocked",
            Self::BadParams => "bad_params",
            Self::AccountFault => "account_fault",
            Self::ChannelRefused => "channel_refused",
            Self::ModelBlocked => "model_blocked",
            Self::WafBlock => "waf_block",
            Self::PromptTooLong => "prompt_too_long",
            Self::ImageInvalid => "image_invalid",
            Self::UpstreamBusy => "upstream_busy",
            Self::Client => "client",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamError {
    pub kind: UpstreamErrKind,
    pub status: u16,
    pub msg: String,
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "upstream {} (http {}): {}",
            self.kind.as_str(),
            self.status,
            self.msg
        )
    }
}

fn set_header(headers: &mut HeaderMap, name: &'static str, val: &str) {
    if val.is_empty() {
        return;
    }
    if let (Ok(k), Ok(v)) = (
        HeaderName::from_bytes(name.as_bytes()),
        HeaderValue::from_str(val),
    ) {
        headers.insert(k, v);
    }
}

fn inject_client_identity_headers(headers: &mut HeaderMap, acc: &WorkBuddyAccount) {
    if account_is_global(acc) {
        return;
    }
    set_header(headers, "x-client-version", client_version_for(acc));
    set_header(headers, "x-client-platform", CLIENT_IDENTITY_PLATFORM);
    set_header(headers, "x-client-product", CLIENT_IDENTITY_PRODUCT);
}

fn inject_identity_headers(headers: &mut HeaderMap, acc: &WorkBuddyAccount) {
    let enterprise = acc.enterprise_id.trim();
    if acc.uid.is_empty() || enterprise.is_empty() {
        return;
    }
    let payload = json!({
        "uin": acc.uid,
        "owner_uin": enterprise,
    });
    if let Ok(raw) = serde_json::to_vec(&payload) {
        set_header(headers, "x-userinfo", &BASE64.encode(raw));
    }
}

fn valid_trace_id(s: &str) -> bool {
    (s.len() == 16 || s.len() == 32) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn private_data_value(acc: &WorkBuddyAccount) -> &'static str {
    if !acc.enterprise_id.trim().is_empty() || account_is_global(acc) {
        "true"
    } else {
        "false"
    }
}

/// Real-device `/v2/chat/completions` outbound headers.
pub fn build_chat_headers(acc: &WorkBuddyAccount, meta: &ChatMeta) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let is_global = account_is_global(acc);

    set_header(&mut headers, "content-type", "application/json");
    set_header(&mut headers, "accept", "application/json");
    set_header(&mut headers, "x-requested-with", "XMLHttpRequest");
    set_header(&mut headers, "x-product", "SaaS");
    set_header(&mut headers, "user-agent", &chat_ua_for(acc));
    inject_client_identity_headers(&mut headers, acc);

    if !acc.access_token.is_empty() {
        set_header(
            &mut headers,
            "authorization",
            &format!("Bearer {}", acc.access_token),
        );
    } else {
        set_header(&mut headers, "x-no-authorization", "true");
    }

    if !acc.uid.is_empty() {
        set_header(&mut headers, "x-user-id", &acc.uid);
    } else {
        set_header(&mut headers, "x-no-user-id", "true");
    }

    if !is_global {
        if !acc.enterprise_id.is_empty() {
            set_header(&mut headers, "x-enterprise-id", &acc.enterprise_id);
        }
        let host = host_of(chat_base_for(acc), "www.workbuddy.cn");
        set_header(&mut headers, "x-domain", &host);
    } else {
        set_header(&mut headers, "x-domain", "www.workbuddy.ai");
    }

    // Attribution headers (official WorkBuddy desktop shape)
    set_header(&mut headers, "x-agent-purpose", "conversation");
    set_header(&mut headers, "x-ide-name", "WorkBuddy");
    set_header(&mut headers, "x-ide-type", "WorkBuddy");
    set_header(&mut headers, "x-ide-version", client_version_for(acc));
    set_header(&mut headers, "x-private-data", private_data_value(acc));

    inject_identity_headers(&mut headers, acc);
    if !acc.device_token.is_empty() {
        set_header(&mut headers, "x-device-token", &acc.device_token);
    }

    // Conversation & distributed tracing headers
    let conv_req_id = if !meta.conversation_request_id.is_empty() {
        meta.conversation_request_id.clone()
    } else {
        new_message_id()
    };
    let message_id = new_message_id();
    let cache_src = if !meta.conversation_id.is_empty() {
        meta.conversation_id.as_str()
    } else {
        meta.cache_key_id.as_str()
    };
    let conv_id = if !meta.conversation_id.is_empty() {
        meta.conversation_id.clone()
    } else if !cache_src.is_empty() {
        build_time_ordered_uuid(&acc.uid, cache_src, "")
    } else {
        new_time_ordered_uuid_v7()
    };
    let root_req_id = if !meta.root_request_id.is_empty() {
        meta.root_request_id.clone()
    } else {
        conv_req_id.clone()
    };
    let trace_id = if !meta.trace_id.is_empty() {
        meta.trace_id.clone()
    } else {
        new_trace_id()
    };
    let b3_trace = if valid_trace_id(&trace_id) {
        trace_id.clone()
    } else {
        message_id.clone()
    };
    let span_id = new_span_id();
    let parent_span_id = new_span_id();
    let acp_conn_id = if !cache_src.is_empty() {
        build_time_ordered_uuid(&acc.uid, cache_src, "acp")
    } else {
        new_time_ordered_uuid_v7()
    };

    set_header(&mut headers, "x-conversation-id", &conv_id);
    set_header(&mut headers, "x-conversation-request-id", &conv_req_id);
    set_header(&mut headers, "x-conversation-message-id", &message_id);
    set_header(&mut headers, "x-request-id", &message_id);
    set_header(&mut headers, "x-root-request-id", &root_req_id);
    set_header(&mut headers, "x-trace-id", &trace_id);
    set_header(&mut headers, "x-b3-traceid", &b3_trace);
    set_header(&mut headers, "x-b3-spanid", &span_id);
    set_header(&mut headers, "x-b3-parentspanid", &parent_span_id);
    set_header(&mut headers, "x-b3-sampled", "1");
    set_header(
        &mut headers,
        "b3",
        &format!("{b3_trace}-{span_id}-1-{parent_span_id}"),
    );
    set_header(
        &mut headers,
        "traceparent",
        &format!("00-{b3_trace}-{span_id}-01"),
    );
    set_header(&mut headers, "acp-connection-id", &acp_conn_id);

    // Client SDK & agent headers
    set_header(&mut headers, "x-agent-type", "main");
    set_header(&mut headers, "x-agent-intent", "craft");
    set_header(&mut headers, "x-agent-runtime", "local-agent");
    set_header(&mut headers, "x-codebuddy-request", "1");
    set_header(&mut headers, "x-stainless-lang", CLIENT_LANG);
    set_header(&mut headers, "x-stainless-package-version", CLIENT_SDK_VER);
    set_header(&mut headers, "x-stainless-os", CLIENT_OS);
    set_header(&mut headers, "x-stainless-arch", CLIENT_ARCH);
    set_header(&mut headers, "x-stainless-runtime", CLIENT_RUNTIME);
    set_header(&mut headers, "x-stainless-runtime-version", CLIENT_NODE_VER);
    set_header(&mut headers, "x-stainless-retry-count", "0");

    // W3C baggage
    set_header(
        &mut headers,
        "baggage",
        &format!(
            "buddyId=,buddyName=,codebuddy.session_id={conv_id},codebuddy.conversation_request_id={conv_req_id}"
        ),
    );

    headers
}

/// Real-device `/v2/plugin/auth/token/refresh` outbound headers.
pub fn build_refresh_headers(acc: &WorkBuddyAccount) -> HeaderMap {
    let mut headers = HeaderMap::new();
    set_header(&mut headers, "content-type", "application/json");
    set_header(&mut headers, "accept", "application/json, text/plain, */*");
    set_header(&mut headers, "x-product", "SaaS");
    set_header(&mut headers, "user-agent", &rpc_ua_for(acc));
    inject_client_identity_headers(&mut headers, acc);

    set_header(&mut headers, "x-refresh-token", &acc.refresh_token);
    set_header(&mut headers, "x-auth-refresh-source", "plugin");
    if !acc.enterprise_id.is_empty() {
        set_header(&mut headers, "x-enterprise-id", &acc.enterprise_id);
    }
    if !acc.access_token.is_empty() {
        set_header(
            &mut headers,
            "authorization",
            &format!("Bearer {}", acc.access_token),
        );
    } else {
        set_header(&mut headers, "x-no-authorization", "true");
    }
    set_header(&mut headers, "x-request-id", &new_trace_id());
    if !acc.uid.is_empty() {
        set_header(&mut headers, "x-user-id", &acc.uid);
    }
    let domain = if !acc.domain.is_empty() {
        acc.domain.clone()
    } else {
        host_of(chat_base_for(acc), "www.workbuddy.cn")
    };
    set_header(&mut headers, "x-domain", &domain);
    headers
}

/// Real-device `/v3/config` headers.
pub fn build_v3_config_headers(acc: &WorkBuddyAccount) -> HeaderMap {
    let mut headers = HeaderMap::new();
    set_header(&mut headers, "accept", "application/json, text/plain, */*");
    set_header(
        &mut headers,
        "authorization",
        &format!("Bearer {}", acc.access_token),
    );
    if !acc.uid.is_empty() {
        set_header(&mut headers, "x-user-id", &acc.uid);
    }
    let domain = if !acc.domain.is_empty() {
        acc.domain.clone()
    } else {
        host_of(chat_base_for(acc), "www.workbuddy.cn")
    };
    set_header(&mut headers, "x-domain", &domain);
    set_header(&mut headers, "x-product", "SaaS");
    set_header(&mut headers, "x-request-id", &new_trace_id());
    set_header(&mut headers, "user-agent", &v3_probe_ua_for(acc));
    inject_client_identity_headers(&mut headers, acc);
    headers
}

/// Real-device `/console/enterprises/personal/models` headers.
pub fn build_enterprise_models_headers(acc: &WorkBuddyAccount) -> HeaderMap {
    let mut headers = HeaderMap::new();
    set_header(&mut headers, "content-type", "application/json");
    set_header(&mut headers, "accept", "application/json, text/plain, */*");
    set_header(&mut headers, "x-product", "SaaS");
    set_header(&mut headers, "user-agent", &rpc_ua_for(acc));
    inject_client_identity_headers(&mut headers, acc);
    set_header(
        &mut headers,
        "authorization",
        &format!("Bearer {}", acc.access_token),
    );
    headers
}

/// Real-device billing / daily-checkin outbound headers (`axios/1.16.1` UA).
pub fn build_billing_headers(acc: &WorkBuddyAccount) -> HeaderMap {
    let mut headers = HeaderMap::new();
    set_header(
        &mut headers,
        "authorization",
        &format!("Bearer {}", acc.access_token),
    );
    set_header(&mut headers, "accept", "application/json");
    set_header(&mut headers, "content-type", "application/json");
    set_header(&mut headers, "x-request-id", &new_trace_id());
    set_header(&mut headers, "user-agent", "axios/1.16.1");
    if !acc.uid.is_empty() {
        set_header(&mut headers, "x-user-id", &acc.uid);
    }
    if !acc.enterprise_id.is_empty() {
        set_header(&mut headers, "x-enterprise-id", &acc.enterprise_id);
        set_header(&mut headers, "x-tenant-id", &acc.enterprise_id);
    }
    let domain = if !acc.domain.is_empty() {
        acc.domain.clone()
    } else {
        host_of(chat_base_for(acc), "www.workbuddy.cn")
    };
    set_header(&mut headers, "x-domain", &domain);
    if !acc.device_token.is_empty() {
        set_header(&mut headers, "x-device-token", &acc.device_token);
    }
    inject_client_identity_headers(&mut headers, acc);
    headers
}

/// Parse optional `[cn:|global:]model` realm routing prefix.
pub fn split_realm_prefix(model: &str) -> (&'static str, &str) {
    if let Some((prefix, bare)) = model.split_once(':') {
        match prefix {
            REALM_CN => return (REALM_CN, bare),
            REALM_GLOBAL => return (REALM_GLOBAL, bare),
            _ => {}
        }
    }
    (REALM_CN, model)
}

/// Extract optional `conversation_id` / `conversationId` from request body (top-level or `metadata`).
pub fn extract_conversation_id(obj: &Map<String, Value>) -> String {
    if let Some(meta) = obj.get("metadata").and_then(|v| v.as_object()) {
        for key in ["conversation_id", "conversationId"] {
            if let Some(s) = meta.get(key).and_then(|v| v.as_str()) {
                if !s.trim().is_empty() {
                    return s.trim().to_string();
                }
            }
        }
    }
    for key in ["conversation_id", "conversationId"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
    }
    String::new()
}

/// Normalize and prepare an OpenAI Chat Completions JSON payload for WorkBuddy upstream.
pub fn prepare_chat_body(
    mut obj: Map<String, Value>,
    is_global: bool,
    uid: &str,
    conv_id: &str,
) -> Value {
    // 1. Strip realm routing prefix from model name
    if let Some(model_str) = obj.get("model").and_then(|v| v.as_str()).map(str::to_string) {
        let (_, bare) = split_realm_prefix(&model_str);
        if bare != model_str {
            obj.insert("model".to_string(), Value::String(bare.to_string()));
        }
    }

    // 2. Always stream upstream (WorkBuddy rejects non-streaming)
    obj.insert("stream".to_string(), Value::Bool(true));

    // 3. Translate max_completion_tokens -> max_tokens & clamp GPT minimum (>=16)
    if let Some(alias) = obj.remove("max_completion_tokens") {
        if !obj.contains_key("max_tokens") {
            if let Some(n) = alias.as_i64().filter(|&n| n > 0) {
                obj.insert("max_tokens".to_string(), json!(n));
            }
        }
    }
    let model_lower = obj
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if model_lower.contains("gpt-") {
        if let Some(n) = obj.get("max_tokens").and_then(|v| v.as_i64()) {
            if n < GPT_MIN_MAX_TOKENS {
                obj.insert("max_tokens".to_string(), json!(GPT_MIN_MAX_TOKENS));
            }
        }
    }

    // 4. Default stream_options.include_usage = true
    if !obj.contains_key("stream_options") {
        obj.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
    }

    // 5. Normalize tool_choice, tool schemas, roles, image_url, stop, reasoning parts
    normalize_tool_choice(&mut obj);
    normalize_tools(&mut obj);
    normalize_messages(&mut obj);
    normalize_stop(&mut obj);

    // 6. Global realm requires leading system message
    if is_global {
        ensure_console_system(&mut obj);
    }

    // 7. Inject deterministic prompt_cache_key if client didn't provide one
    let has_cache_key = obj
        .get("prompt_cache_key")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    if !has_cache_key {
        let key_conv = if !conv_id.is_empty() {
            conv_id.to_string()
        } else {
            extract_conversation_id(&obj)
        };
        obj.insert(
            "prompt_cache_key".to_string(),
            Value::String(build_cache_key_v4(uid, &key_conv)),
        );
    }

    Value::Object(obj)
}

fn normalize_tool_choice(obj: &mut Map<String, Value>) {
    let Some(tc) = obj.get("tool_choice").cloned() else {
        return;
    };
    match tc {
        Value::String(s) => {
            if s.trim().eq_ignore_ascii_case("none") {
                obj.remove("tool_choice");
                obj.remove("tools");
                obj.remove("functions");
            }
        }
        Value::Object(map) => {
            let typ = map
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            match typ.as_str() {
                "none" => {
                    obj.remove("tool_choice");
                    obj.remove("tools");
                    obj.remove("functions");
                }
                "auto" | "required" => {
                    obj.insert("tool_choice".to_string(), Value::String(typ));
                }
                "function" => {
                    let name = map
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .or_else(|| map.get("name").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .trim();
                    let resolved = if name.is_empty() { "auto" } else { name };
                    obj.insert("tool_choice".to_string(), Value::String(resolved.to_string()));
                }
                _ => {
                    obj.remove("tool_choice");
                }
            }
        }
        _ => {
            obj.remove("tool_choice");
        }
    }
}

fn normalize_tools(obj: &mut Map<String, Value>) {
    let Some(tools) = obj.get_mut("tools").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for tool in tools.iter_mut() {
        let Some(tool_obj) = tool.as_object_mut() else {
            continue;
        };
        if let Some(fn_obj) = tool_obj.get_mut("function").and_then(|v| v.as_object_mut()) {
            let params = fn_obj
                .remove("parameters")
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            fn_obj.insert("parameters".to_string(), normalize_tool_parameters(params));
        }
        if let Some(params) = tool_obj.remove("parameters") {
            tool_obj.insert("parameters".to_string(), normalize_tool_parameters(params));
        }
    }
}

fn normalize_tool_parameters(node: Value) -> Value {
    let mut map = match node {
        Value::Object(m) => m,
        _ => {
            let mut m = Map::new();
            m.insert("type".to_string(), Value::String("object".to_string()));
            m.insert("properties".to_string(), Value::Object(Map::new()));
            return Value::Object(m);
        }
    };
    map.remove("$ref");
    let t = map
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let norm_t = normalize_type_alias(&t);
    if norm_t.is_empty() {
        map.insert("type".to_string(), Value::String("object".to_string()));
    } else {
        map.insert("type".to_string(), Value::String(norm_t.to_string()));
    }
    if map.get("type").and_then(|v| v.as_str()) == Some("object")
        && !map.get("properties").is_some_and(|v| v.is_object())
    {
        map.insert("properties".to_string(), Value::Object(Map::new()));
    }
    let mut val = Value::Object(map);
    normalize_schema_tree(&mut val);
    val
}

fn normalize_type_alias(t: &str) -> &str {
    match t {
        "str" => "string",
        "int" => "integer",
        "float" => "number",
        "bool" => "boolean",
        "dict" => "object",
        "list" => "array",
        other => other,
    }
}

fn is_schema_data_key(k: &str) -> bool {
    matches!(k, "default" | "const" | "enum" | "example" | "examples")
}

fn is_schema_container_key(k: &str) -> bool {
    matches!(
        k,
        "properties"
            | "patternProperties"
            | "$defs"
            | "definitions"
            | "dependentSchemas"
            | "dependentRequired"
            | "dependencies"
    )
}

fn normalize_schema_tree(node: &mut Value) {
    match node {
        Value::Object(map) => {
            // Strip null non-data keywords
            let null_keys: Vec<String> = map
                .iter()
                .filter(|(k, v)| v.is_null() && !is_schema_data_key(k))
                .map(|(k, _)| k.clone())
                .collect();
            for k in null_keys {
                map.remove(&k);
            }

            // Normalize type alias
            if let Some(t) = map.get("type").and_then(|v| v.as_str()).map(str::to_string) {
                let lower = t.trim().to_ascii_lowercase();
                let fixed = normalize_type_alias(&lower);
                if fixed.is_empty() {
                    if map.contains_key("properties") {
                        map.insert("type".to_string(), Value::String("object".to_string()));
                    }
                } else if fixed != t {
                    map.insert("type".to_string(), Value::String(fixed.to_string()));
                }
            }

            // Clean required array (dedup, non-empty strings only)
            if let Some(req) = map.get("required").cloned() {
                if let Value::Array(arr) = req {
                    let mut seen = HashSet::new();
                    let cleaned: Vec<Value> = arr
                        .into_iter()
                        .filter_map(|item| item.as_str().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
                        .map(Value::String)
                        .collect();
                    if cleaned.is_empty() {
                        map.remove("required");
                    } else {
                        map.insert("required".to_string(), Value::Array(cleaned));
                    }
                } else {
                    map.remove("required");
                }
            }

            // Fix non-standard `\_` escape in pattern / patternProperties
            if let Some(p) = map.get("pattern").and_then(|v| v.as_str()).map(str::to_string) {
                if p.contains(r"\_") {
                    map.insert("pattern".to_string(), Value::String(p.replace(r"\_", "_")));
                }
            }
            if let Some(Value::Object(pp)) = map.get_mut("patternProperties") {
                let keys: Vec<String> = pp.keys().cloned().collect();
                if keys.iter().any(|k| k.contains(r"\_")) {
                    let mut rebuilt = Map::new();
                    for k in keys {
                        if let Some(v) = pp.remove(&k) {
                            rebuilt.insert(k.replace(r"\_", "_"), v);
                        }
                    }
                    *pp = rebuilt;
                }
            }

            // Recurse
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if is_schema_data_key(&k) {
                    continue;
                }
                if is_schema_container_key(&k) {
                    if let Some(Value::Object(container)) = map.get_mut(&k) {
                        let ckeys: Vec<String> = container.keys().cloned().collect();
                        for ck in ckeys {
                            if container.get(&ck).is_some_and(|v| v.is_null()) {
                                container.remove(&ck);
                            } else if let Some(child) = container.get_mut(&ck) {
                                normalize_schema_tree(child);
                            }
                        }
                    }
                    continue;
                }
                if let Some(v) = map.get_mut(&k) {
                    normalize_schema_tree(v);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                normalize_schema_tree(item);
            }
        }
        _ => {}
    }
}

fn normalize_messages(obj: &mut Map<String, Value>) {
    let Some(msgs) = obj.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };

    let mut out = Vec::with_capacity(msgs.len());
    for mut msg in msgs.drain(..) {
        if let Some(m) = msg.as_object_mut() {
            // developer -> system
            if let Some(role) = m.get("role").and_then(|v| v.as_str()) {
                if role.trim().eq_ignore_ascii_case("developer") {
                    m.insert("role".to_string(), Value::String("system".to_string()));
                }
            }

            // Normalize content parts: image_url string -> object; drop reasoning_text/reasoning
            let mut drop_whole_msg = false;
            let has_tool_calls = m.contains_key("tool_calls");
            if let Some(parts) = m.get_mut("content").and_then(|v| v.as_array_mut()) {
                let mut kept = Vec::with_capacity(parts.len());
                for mut part in parts.drain(..) {
                    if let Some(po) = part.as_object_mut() {
                        let ptype = po
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if ptype == "reasoning_text" || ptype == "reasoning" {
                            continue;
                        }
                        if ptype == "image_url" {
                            if let Some(url_str) =
                                po.get("image_url").and_then(|v| v.as_str()).map(str::to_string)
                            {
                                if !url_str.is_empty() {
                                    po.insert("image_url".to_string(), json!({ "url": url_str }));
                                }
                            }
                        }
                        if ptype == "text"
                            && po
                                .get("text")
                                .and_then(|v| v.as_str())
                                .is_some_and(|s| s.is_empty())
                        {
                            po.insert("text".to_string(), Value::String(" ".to_string()));
                        }
                    }
                    kept.push(part);
                }
                if kept.is_empty() && !has_tool_calls {
                    drop_whole_msg = true;
                } else {
                    *parts = kept;
                }
            }
            if drop_whole_msg {
                continue;
            }
        }
        out.push(msg);
    }
    *msgs = out;
}

fn normalize_stop(obj: &mut Map<String, Value>) {
    if let Some(s) = obj.get("stop").and_then(|v| v.as_str()).map(str::to_string) {
        obj.insert("stop".to_string(), Value::Array(vec![Value::String(s)]));
    }
}

fn ensure_console_system(obj: &mut Map<String, Value>) {
    let Some(msgs) = obj.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };
    if msgs.is_empty() {
        return;
    }
    let first_is_system = msgs[0]
        .get("role")
        .and_then(|v| v.as_str())
        .is_some_and(|r| r.trim().eq_ignore_ascii_case("system"));
    if !first_is_system {
        msgs.insert(
            0,
            json!({
                "role": "system",
                "content": CONSOLE_SYSTEM_FALLBACK
            }),
        );
    }
}

/// Gzip chat body when >= 2KB (matching desktop WorkBuddy client).
pub fn maybe_gzip_chat_body(raw: Vec<u8>) -> (Vec<u8>, bool) {
    if raw.len() < GZIP_MIN_CHAT_BODY {
        return (raw, false);
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    if encoder.write_all(&raw).is_ok() {
        if let Ok(compressed) = encoder.finish() {
            return (compressed, true);
        }
    }
    (raw, false)
}

fn has_business_envelope(body: &str) -> bool {
    body.contains("\"code\":") || body.contains("\"msg\":")
}

fn has_business_code(body: &str, want: &str) -> bool {
    let Ok(val) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let check = |c: &Value| -> bool {
        match c {
            Value::String(s) => s.trim() == want,
            Value::Number(n) => n.to_string() == want,
            _ => false,
        }
    };
    if let Some(c) = val.get("code") {
        if check(c) {
            return true;
        }
    }
    if let Some(c) = val.get("error").and_then(|e| e.get("code")) {
        if check(c) {
            return true;
        }
    }
    false
}

/// Classify upstream HTTP >= 400 error into WorkBuddy2API error taxonomy.
pub fn classify_upstream_error(status: u16, body: &str) -> UpstreamErrKind {
    let lower = body.to_ascii_lowercase();

    if status == 401 {
        if has_business_code(body, "12153") || !has_business_envelope(body) {
            return UpstreamErrKind::SessionDead;
        }
        return UpstreamErrKind::SessionDead;
    }

    if status == 402 {
        return UpstreamErrKind::HardCredit;
    }

    if status == 403 && !has_business_envelope(body) {
        return UpstreamErrKind::WafBlock;
    }

    // Channel refused (11-128 + request illegal) vs account restricted (11140 + request illegal)
    if lower.contains("request illegal") {
        if has_business_code(body, "11-128") {
            return UpstreamErrKind::ChannelRefused;
        }
        if has_business_code(body, "11140") {
            return UpstreamErrKind::AccountFault;
        }
    }

    // Queue full / upstream capacity
    if has_business_code(body, "6021")
        || has_business_code(body, "6020")
        || lower.contains("queue.full")
        || lower.contains("queue full")
    {
        return UpstreamErrKind::UpstreamBusy;
    }

    // Content blocked
    if lower.contains("blocked by security policy")
        || lower.contains("unapproved channel")
        || lower.contains("illegal api invocation")
    {
        return UpstreamErrKind::ContentBlocked;
    }

    // Prompt too long
    if (status == 400 || status == 404 || status == 413)
        && (has_business_code(body, "11115") || lower.contains("prompt is too long"))
    {
        return UpstreamErrKind::PromptTooLong;
    }

    // Invalid image
    if status == 400
        && (has_business_code(body, "11135")
            || lower.contains("invalid image_url content")
            || lower.contains("invalid_image_data")
            || lower.contains("replace the image"))
    {
        return UpstreamErrKind::ImageInvalid;
    }

    // Bad params / tool schema rejected
    if status == 400
        && (has_business_code(body, "11101")
            || has_business_code(body, "11129")
            || has_business_code(body, "11152")
            || lower.contains("unmarshal chat params failed"))
    {
        return UpstreamErrKind::BadParams;
    }

    // Model blocked / missing
    if (status == 400 || status == 404) && has_business_code(body, "11102") {
        return UpstreamErrKind::ModelBlocked;
    }

    // Account trial not activated
    if lower.contains("trial not activated") || lower.contains("trial version is not yet activated")
    {
        return UpstreamErrKind::AccountFault;
    }

    // Hard credit exhaustion
    let hard_markers = [
        "insufficient credit",
        "no credit",
        "credit exhausted",
        "credits exhausted",
        "out of credit",
        "quota exceeded",
        "quota exhaust",
        "payment required",
        "credit not enough",
        "not enough credit",
        "积分不足",
        "额度不足",
        "余额不足",
        "积分用完",
        "额度用尽",
        "没有积分",
    ];
    if hard_markers
        .iter()
        .any(|m| lower.contains(m) || body.contains(m))
    {
        return UpstreamErrKind::HardCredit;
    }

    // Soft rate limit (HTTP 429 or business codes 6004 / 14003 / rate-limit text)
    if status == 429
        || has_business_code(body, "6004")
        || has_business_code(body, "14003")
        || has_business_code(body, "11134")
        || lower.contains("rate limit")
        || lower.contains("rate-limit")
        || lower.contains("too many requests")
    {
        return UpstreamErrKind::SoftRate;
    }

    if status == 404 {
        return UpstreamErrKind::NotFound;
    }
    if status >= 500 {
        return UpstreamErrKind::Server;
    }
    UpstreamErrKind::Client
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepares_chat_body_with_all_upstream_compat_rules() {
        let input = json!({
            "model": "cn:deepseek-chat",
            "stream": false,
            "max_completion_tokens": 4096,
            "stop": "\n\n",
            "tool_choice": { "type": "function", "function": { "name": "web_search" } },
            "tools": [{
                "type": "function",
                "function": {
                    "name": "web_search",
                    "parameters": {
                        "type": "dict",
                        "properties": {
                            "id": { "type": "str", "pattern": "^agent\\_run\\_", "description": null }
                        },
                        "required": ["id", "id", ""]
                    }
                }
            }],
            "messages": [
                { "role": "developer", "content": "Be concise" },
                { "role": "user", "content": [{ "type": "image_url", "image_url": "data:image/png;base64,AAA" }] }
            ]
        });

        let out = prepare_chat_body(
            input.as_object().unwrap().clone(),
            false,
            "uid-1",
            "conv-1",
        );

        assert_eq!(out["model"], "deepseek-chat");
        assert_eq!(out["stream"], true);
        assert_eq!(out["max_tokens"], 4096);
        assert!(out.get("max_completion_tokens").is_none());
        assert_eq!(out["stop"], json!(["\n\n"]));
        assert_eq!(out["tool_choice"], "web_search");
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(
            out["messages"][1]["content"][0]["image_url"]["url"],
            "data:image/png;base64,AAA"
        );
        assert_eq!(
            out["tools"][0]["function"]["parameters"]["properties"]["id"]["pattern"],
            "^agent_run_"
        );
        assert_eq!(
            out["tools"][0]["function"]["parameters"]["required"],
            json!(["id"])
        );
        assert!(out["prompt_cache_key"].as_str().unwrap().len() == 36);
    }

    #[test]
    fn classifies_upstream_errors_accurately() {
        assert_eq!(
            classify_upstream_error(400, r#"{"code":"11-128","msg":"request illegal"}"#),
            UpstreamErrKind::ChannelRefused
        );
        assert_eq!(
            classify_upstream_error(403, r#"{"code":11140,"msg":"request illegal"}"#),
            UpstreamErrKind::AccountFault
        );
        assert_eq!(
            classify_upstream_error(403, "<html>WAF blocked</html>"),
            UpstreamErrKind::WafBlock
        );
        assert_eq!(
            classify_upstream_error(429, r#"{"code":6004,"msg":"rate limited"}"#),
            UpstreamErrKind::SoftRate
        );
        assert_eq!(
            classify_upstream_error(402, r#"{"msg":"余额不足"}"#),
            UpstreamErrKind::HardCredit
        );
        assert_eq!(
            classify_upstream_error(401, r#"{"code":12153,"msg":"offline"}"#),
            UpstreamErrKind::SessionDead
        );
    }
}
