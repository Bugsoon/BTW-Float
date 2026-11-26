//! HTTP/1.1-only upstream client for WorkBuddy / CodeBuddy:
//! - Model discovery (`/v3/config` with official client-picker filtering + `/console/enterprises/personal/models` fallback)
//! - Token refresh (`/v2/plugin/auth/token/refresh`)
//! - Streaming chat completions (`/v2/chat/completions`)

use crate::config::WorkBuddyAccount;
use crate::win_native;
use crate::workbuddy::protocol::{
    build_billing_headers, build_chat_headers, build_enterprise_models_headers,
    build_refresh_headers, build_v3_config_headers, classify_upstream_error, maybe_gzip_chat_body,
    ChatMeta, UpstreamErrKind, UpstreamError,
};
use crate::workbuddy::site::{account_is_global, chat_base_for, unix_now_secs};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const AUTO_MODE_IDS: &[&str] = &["auto", "auto-chat"];
const PICK_PRESET_IDS: &[&str] = &["fast-model", "balanced-model", "deep-model"];
const REFRESH_EXPIRES_IN_MAX_SECS: i64 = 10 * 365 * 24 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModel {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub vision: bool,
    pub tool_calling: bool,
    pub context_window: i64,
    pub max_tokens: i64,
}

#[derive(Debug, Clone)]
pub struct RefreshTokenOutput {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub domain: Option<String>,
    pub expires_at: Option<i64>,
}

/// Build an HTTP/1.1-only reqwest client (disables h2 to prevent half-dead stream hangs)
/// honoring the Windows system proxy when configured.
pub fn build_upstream_http_client(timeout_secs: u64, stream_mode: bool) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .http1_only()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(60))
        .pool_max_idle_per_host(8);

    if !stream_mode {
        builder = builder.timeout(Duration::from_secs(timeout_secs));
    }

    if let Some(proxy_url) = win_native::get_system_proxy_url() {
        if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
            builder = builder.proxy(proxy);
        }
    }

    builder
        .build()
        .map_err(|e| format!("创建 WorkBuddy 网络客户端失败: {e}"))
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope {
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Value,
}

/// Refresh an account's access token via `/v2/plugin/auth/token/refresh`.
pub async fn refresh_account_token(
    client: &reqwest::Client,
    acc: &WorkBuddyAccount,
) -> Result<RefreshTokenOutput, String> {
    if acc.refresh_token.trim().is_empty() {
        return Err("缺少 refresh_token，请重新登录".to_string());
    }

    let endpoint = format!("{}/v2/plugin/auth/token/refresh", chat_base_for(acc));
    let headers = build_refresh_headers(acc);

    let resp = client
        .post(&endpoint)
        .headers(headers)
        .body("{}")
        .send()
        .await
        .map_err(|e| format!("刷新令牌请求失败: {e}"))?;

    let status = resp.status();
    let raw = resp
        .text()
        .await
        .map_err(|e| format!("读取刷新响应失败: {e}"))?;

    if !status.is_success() {
        let snippet: String = raw.chars().take(180).collect();
        return Err(format!("刷新令牌返回 HTTP {status}: {snippet}"));
    }

    let env: ApiEnvelope =
        serde_json::from_str(&raw).map_err(|e| format!("解析刷新响应失败: {e}"))?;
    if env.code != 0 {
        return Err(format!(
            "刷新令牌失败 (code={}): {}",
            env.code,
            if env.msg.is_empty() { "需要重新登录" } else { &env.msg }
        ));
    }

    let access_token = env
        .data
        .get("accessToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if access_token.is_empty() {
        return Err("刷新响应缺少 accessToken，请重新登录".to_string());
    }

    let refresh_token = env
        .data
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let domain = env
        .data
        .get("domain")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let expires_in = env
        .data
        .get("expiresIn")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let expires_at = if expires_in > 0 && expires_in < REFRESH_EXPIRES_IN_MAX_SECS {
        Some(unix_now_secs() + expires_in)
    } else {
        None
    };

    Ok(RefreshTokenOutput {
        access_token,
        refresh_token,
        domain,
        expires_at,
    })
}

/// Send streaming chat completions request to WorkBuddy `/v2/chat/completions`.
pub async fn send_chat_stream(
    client: &reqwest::Client,
    acc: &WorkBuddyAccount,
    prepared_body: &Value,
    meta: &ChatMeta,
) -> Result<reqwest::Response, UpstreamError> {
    let endpoint = format!("{}/v2/chat/completions", chat_base_for(acc));
    let raw_bytes = serde_json::to_vec(prepared_body).map_err(|e| UpstreamError {
        kind: UpstreamErrKind::BadParams,
        status: 400,
        msg: format!("序列化请求体失败: {e}"),
    })?;

    let (body_bytes, gzipped) = maybe_gzip_chat_body(raw_bytes);
    let mut headers = build_chat_headers(acc, meta);
    if gzipped {
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));
    }

    let resp = client
        .post(&endpoint)
        .headers(headers)
        .body(body_bytes)
        .send()
        .await
        .map_err(|e| UpstreamError {
            kind: UpstreamErrKind::Server,
            status: 502,
            msg: format!("上游连接失败: {e}"),
        })?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let raw_body = resp
            .bytes()
            .await
            .map(|b| b.to_vec())
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&raw_body);
        let kind = classify_upstream_error(status, &text);
        let msg: String = text.chars().take(240).collect();
        return Err(UpstreamError { kind, status, msg });
    }

    Ok(resp)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DynModelEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    vendor: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    max_input_tokens: i64,
    #[serde(default)]
    max_output_tokens: i64,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    supports_images: Option<bool>,
    #[serde(default)]
    supports_tool_call: Option<bool>,
}

impl DynModelEntry {
    fn to_upstream_model(&self) -> UpstreamModel {
        let id = self.id.trim().to_string();
        let name = if self.name.trim().is_empty() {
            id.clone()
        } else {
            self.name.trim().to_string()
        };
        let lower = id.to_ascii_lowercase();
        let vision = self.supports_images.unwrap_or_else(|| {
            lower.contains("gpt-4o")
                || lower.contains("gpt-5")
                || lower.contains("gpt-6")
                || lower.contains("gemini")
                || lower.contains("claude")
                || lower.contains("-vl")
                || lower.contains("vision")
                || lower.contains("kimi-k2.5")
        });
        let tool_calling = self.supports_tool_call.unwrap_or(true);

        UpstreamModel {
            id,
            name,
            vendor: self.vendor.trim().to_string(),
            vision,
            tool_calling,
            context_window: self.max_input_tokens,
            max_tokens: self.max_output_tokens,
        }
    }
}

fn is_non_chat_model(id: &str, max_output_tokens: i64, tags: &[String]) -> bool {
    let lower = id.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    if lower.starts_with("nes-")
        || lower.starts_with("completion-")
        || lower.starts_with("codewise-")
    {
        return true;
    }
    if max_output_tokens > 0 && max_output_tokens <= 256 {
        return true;
    }
    tags.iter().any(|t| {
        let tl = t.trim().to_ascii_lowercase();
        tl.contains("text-to-image")
            || tl.contains("image-to-image")
            || tl.contains("image_generation")
            || tl.contains("video")
    })
}

fn is_auto_mode_id(id: &str) -> bool {
    AUTO_MODE_IDS.contains(&id.trim())
}

#[derive(Debug, Deserialize)]
struct V3Agent {
    #[serde(default)]
    name: String,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn pick_default_agent(agents: &[V3Agent]) -> Option<&V3Agent> {
    if let Some(a) = agents.iter().find(|ag| {
        ag.tags
            .iter()
            .any(|t| t.trim().eq_ignore_ascii_case("default"))
            && !ag.models.is_empty()
    }) {
        return Some(a);
    }
    if let Some(a) = agents
        .iter()
        .find(|ag| ag.name.trim().eq_ignore_ascii_case("cli") && !ag.models.is_empty())
    {
        return Some(a);
    }
    agents.iter().find(|ag| !ag.models.is_empty())
}

/// Fetch chat models from `/v3/config` (primary) with `/console/enterprises/personal/models` fallback.
pub async fn fetch_account_models(
    client: &reqwest::Client,
    acc: &WorkBuddyAccount,
) -> Result<Vec<UpstreamModel>, String> {
    match fetch_v3_models(client, acc).await {
        Ok(models) if !models.is_empty() => Ok(models),
        v3_err => {
            if !account_is_global(acc) {
                if let Ok(ent_models) = fetch_enterprise_models(client, acc).await {
                    if !ent_models.is_empty() {
                        return Ok(ent_models);
                    }
                }
            }
            v3_err
        }
    }
}

async fn fetch_v3_models(
    client: &reqwest::Client,
    acc: &WorkBuddyAccount,
) -> Result<Vec<UpstreamModel>, String> {
    let endpoint = format!("{}/v3/config", chat_base_for(acc));
    let headers = build_v3_config_headers(acc);

    let resp = client
        .get(&endpoint)
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("拉取 /v3/config 失败: {e}"))?;

    let status = resp.status();
    let raw = resp
        .text()
        .await
        .map_err(|e| format!("读取 /v3/config 失败: {e}"))?;
    if !status.is_success() {
        let snippet: String = raw.chars().take(160).collect();
        return Err(format!("/v3/config 返回 HTTP {status}: {snippet}"));
    }

    let env: ApiEnvelope =
        serde_json::from_str(&raw).map_err(|e| format!("解析 /v3/config JSON 失败: {e}"))?;
    if env.code != 0 {
        return Err(format!("/v3/config 业务错误 code={}: {}", env.code, env.msg));
    }

    let raw_models: Vec<DynModelEntry> = env
        .data
        .get("models")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let mut by_id: HashMap<String, (UpstreamModel, Vec<String>)> = HashMap::new();
    let mut all_order: Vec<String> = Vec::new();
    for entry in raw_models {
        let id = entry.id.trim().to_string();
        if id.is_empty() || entry.disabled {
            continue;
        }
        let tags = entry.tags.clone();
        let model = entry.to_upstream_model();
        if !by_id.contains_key(&id) {
            all_order.push(id.clone());
        }
        by_id.insert(id, (model, tags));
    }

    // Merge trial banner models if present
    if let Some(banners) = env
        .data
        .pointer("/productFeaturesConfig/ModelTrialBanner/banners")
        .and_then(|v| v.as_array())
    {
        for b in banners {
            let id = b
                .get("modelId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if id.is_empty() || by_id.contains_key(id) {
                continue;
            }
            let target = b
                .get("targetModelId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let model = if let Some((base, _)) = by_id.get(target) {
                let mut m = base.clone();
                m.id = id.to_string();
                m
            } else {
                UpstreamModel {
                    id: id.to_string(),
                    name: id.to_string(),
                    vendor: String::new(),
                    vision: false,
                    tool_calling: true,
                    context_window: 0,
                    max_tokens: 0,
                }
            };
            all_order.push(id.to_string());
            by_id.insert(id.to_string(), (model, Vec::new()));
        }
    }

    // Paid IDs hidden by ModelRateLimitCap
    let mut hidden_paid: HashSet<String> = HashSet::new();
    if let Some(lines) = env
        .data
        .pointer("/productFeaturesConfig/ModelRateLimitCap/lines")
        .and_then(|v| v.as_array())
    {
        for line in lines {
            if let Some(paid_id) = line.get("paidId").and_then(|v| v.as_str()) {
                let p = paid_id.trim();
                if !p.is_empty() {
                    hidden_paid.insert(p.to_string());
                }
            }
        }
    }

    // Official client picker menu order
    let agents: Vec<V3Agent> = env
        .data
        .get("agents")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if let Some(default_agent) = pick_default_agent(&agents) {
        let in_order: HashSet<&str> = default_agent.models.iter().map(|s| s.trim()).collect();
        let mut try_push = |id: &str| {
            let clean = id.trim();
            if clean.is_empty()
                || seen.contains(clean)
                || is_auto_mode_id(clean)
                || hidden_paid.contains(clean)
            {
                return;
            }
            if let Some((model, tags)) = by_id.get(clean) {
                if !is_non_chat_model(&model.id, model.max_tokens, tags) {
                    seen.insert(clean.to_string());
                    out.push(model.clone());
                }
            }
        };

        for preset in PICK_PRESET_IDS {
            if !in_order.contains(*preset) {
                try_push(preset);
            }
        }
        for id in &default_agent.models {
            try_push(id);
        }
    } else {
        for id in all_order {
            if seen.contains(&id) || is_auto_mode_id(&id) || hidden_paid.contains(&id) {
                continue;
            }
            if let Some((model, tags)) = by_id.get(&id) {
                if !is_non_chat_model(&model.id, model.max_tokens, tags) {
                    seen.insert(id);
                    out.push(model.clone());
                }
            }
        }
    }

    if out.is_empty() {
        return Err("/v3/config 未返回可用对话模型".to_string());
    }
    Ok(out)
}

async fn fetch_enterprise_models(
    client: &reqwest::Client,
    acc: &WorkBuddyAccount,
) -> Result<Vec<UpstreamModel>, String> {
    let endpoint = format!(
        "{}/console/enterprises/personal/models",
        chat_base_for(acc)
    );
    let headers = build_enterprise_models_headers(acc);

    let resp = client
        .get(&endpoint)
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("拉取企业模型列表失败: {e}"))?;

    let status = resp.status();
    let raw = resp
        .text()
        .await
        .map_err(|e| format!("读取企业模型列表失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("企业模型接口返回 HTTP {status}"));
    }

    let env: ApiEnvelope =
        serde_json::from_str(&raw).map_err(|e| format!("解析企业模型 JSON 失败: {e}"))?;
    if env.code != 0 {
        return Err(format!("企业模型接口 code={}", env.code));
    }

    let raw_models: Vec<DynModelEntry> = env
        .data
        .get("models")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let agents: Vec<V3Agent> = env
        .data
        .get("agents")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let mut map: HashMap<String, UpstreamModel> = HashMap::new();
    for m in raw_models {
        if m.disabled || is_non_chat_model(&m.id, m.max_output_tokens, &m.tags) {
            continue;
        }
        map.insert(m.id.trim().to_string(), m.to_upstream_model());
    }

    let cli_ids = agents
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("cli"))
        .map(|a| a.models.as_slice())
        .unwrap_or(&[]);

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for id in cli_ids {
        let clean = id.trim();
        if is_auto_mode_id(clean) || !seen.insert(clean.to_string()) {
            continue;
        }
        if let Some(m) = map.get(clean) {
            out.push(m.clone());
        }
    }
    if out.is_empty() {
        return Err("企业模型列表为空".to_string());
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckinState {
    Checked,
    Already,
    Pending,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinReceipt {
    pub uid: String,
    pub state: CheckinState,
    pub credit: Option<i64>,
    pub streak_days: Option<i64>,
    pub message: String,
}

const CHECKIN_FAILURE_MARKERS: &[&str] = &["失败", "fail", "not started", "未开启", "不支持"];
const ALREADY_CHECKIN_MARKERS: &[&str] = &[
    "已签到",
    "今天已签",
    "今日已签",
    "重复签到",
    "签过",
    "already checked",
    "already sign",
    "already checkin",
];
const BILLING_PROCESSING_MARKERS: &[&str] = &["处理中", "请勿重复操作", "processing"];

fn contains_any_ci(lower: &str, markers: &[&str]) -> bool {
    markers
        .iter()
        .any(|m| lower.contains(&m.to_ascii_lowercase()))
}

/// Four-state check-in receipt classification (matches `misakano7545/workbuddy2api-panel/internal/upstream/checkin_receipt.go`).
/// Priority order: `pending` -> `failed` -> `already` -> `checked`.
pub fn classify_checkin_response(uid: &str, http_status: u16, raw: &str) -> CheckinReceipt {
    let parsed: Option<Value> = serde_json::from_str(raw).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let msg = parsed
        .as_ref()
        .and_then(|v| {
            v.get("msg")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
        })
        .unwrap_or("")
        .trim()
        .to_string();

    let is_err = http_status >= 400 || code != 0;
    let text_for_match = if msg.is_empty() {
        raw.to_ascii_lowercase()
    } else {
        msg.to_ascii_lowercase()
    };

    if is_err {
        let state = if contains_any_ci(&text_for_match, BILLING_PROCESSING_MARKERS) {
            CheckinState::Pending
        } else if contains_any_ci(&text_for_match, CHECKIN_FAILURE_MARKERS) {
            CheckinState::Failed
        } else if contains_any_ci(&text_for_match, ALREADY_CHECKIN_MARKERS) {
            CheckinState::Already
        } else {
            CheckinState::Failed
        };
        let display_msg = if !msg.is_empty() {
            msg
        } else if state == CheckinState::Already {
            "今日已签到".to_string()
        } else {
            format!("HTTP {http_status}")
        };
        return CheckinReceipt {
            uid: uid.to_string(),
            state,
            credit: None,
            streak_days: None,
            message: display_msg,
        };
    }

    if contains_any_ci(&text_for_match, CHECKIN_FAILURE_MARKERS) {
        return CheckinReceipt {
            uid: uid.to_string(),
            state: CheckinState::Failed,
            credit: None,
            streak_days: None,
            message: if msg.is_empty() {
                "签到失败".to_string()
            } else {
                msg
            },
        };
    }
    if contains_any_ci(&text_for_match, ALREADY_CHECKIN_MARKERS) {
        return CheckinReceipt {
            uid: uid.to_string(),
            state: CheckinState::Already,
            credit: None,
            streak_days: None,
            message: if msg.is_empty() {
                "今日已签到".to_string()
            } else {
                msg
            },
        };
    }

    // Parse flat body or `data` envelope for real awarded credits & streak
    let data_obj = parsed
        .as_ref()
        .and_then(|v| v.get("data"))
        .filter(|d| d.is_object())
        .or(parsed.as_ref());

    let credit = data_obj
        .and_then(|d| d.get("credit"))
        .and_then(|v| v.as_i64());
    let streak_days = data_obj
        .and_then(|d| d.get("streak_days").or_else(|| d.get("streakDays")))
        .and_then(|v| v.as_i64())
        .filter(|&n| n > 0);

    let message = match (credit, streak_days) {
        (Some(c), Some(s)) => format!("签到成功 (+{c} 积分 · 连续 {s} 天)"),
        (Some(c), None) => format!("签到成功 (+{c} 积分)"),
        (None, Some(s)) => format!("签到成功 (连续 {s} 天)"),
        (None, None) => "签到成功".to_string(),
    };

    CheckinReceipt {
        uid: uid.to_string(),
        state: CheckinState::Checked,
        credit,
        streak_days,
        message,
    }
}

/// Execute daily check-in for a CN personal account (`POST /v2/billing/meter/daily-checkin`).
/// Enterprise & global accounts are cleanly skipped (not supported upstream).
pub async fn daily_checkin(
    client: &reqwest::Client,
    acc: &WorkBuddyAccount,
) -> Result<CheckinReceipt, String> {
    if account_is_global(acc) {
        return Ok(CheckinReceipt {
            uid: acc.uid.clone(),
            state: CheckinState::Skipped,
            credit: None,
            streak_days: None,
            message: "国际版无每日签到".to_string(),
        });
    }
    if !acc.enterprise_id.trim().is_empty() {
        return Ok(CheckinReceipt {
            uid: acc.uid.clone(),
            state: CheckinState::Skipped,
            credit: None,
            streak_days: None,
            message: "企业账号不支持每日签到".to_string(),
        });
    }

    let endpoint = format!("{}/v2/billing/meter/daily-checkin", chat_base_for(acc));
    let mut last_err = String::new();

    // Bounded retry (up to 2 attempts) for transient 5xx / transport errors
    for attempt in 0..2 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        let headers = build_billing_headers(acc);
        match client.post(&endpoint).headers(headers).body("{}").send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let raw = resp.text().await.unwrap_or_default();
                if status >= 500 && attempt == 0 {
                    last_err = format!("签到接口 HTTP {status}");
                    continue;
                }
                return Ok(classify_checkin_response(&acc.uid, status, &raw));
            }
            Err(e) => {
                last_err = format!("签到网络请求失败: {e}");
            }
        }
    }

    Err(last_err)
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditBalance {
    pub remain: i64,
    pub total: i64,
    pub unlimited: bool,
}

fn format_cst_datetime(unix_secs: i64) -> String {
    let cst_secs = unix_secs + 8 * 3600;
    let days = cst_secs.div_euclid(86_400);
    let secs_of_day = cst_secs.rem_euclid(86_400);
    let h = secs_of_day / 3600;
    let m = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mo <= 2 { y + 1 } else { y };
    format!("{year:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

fn parse_personal_credits(raw: &str) -> Result<CreditBalance, String> {
    let root: Value = serde_json::from_str(raw).map_err(|e| format!("解析积分响应失败: {e}"))?;
    if let Some(code) = root.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = root.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            return Err(format!("查询积分失败 (code={code}): {msg}"));
        }
    }
    let accounts = root
        .pointer("/data/Response/Data/Accounts")
        .or_else(|| root.pointer("/Response/Data/Accounts"))
        .and_then(|v| v.as_array());

    let Some(packs) = accounts else {
        return Ok(CreditBalance {
            remain: 0,
            total: 0,
            unlimited: false,
        });
    };

    let mut sum_remain: i64 = 0;
    let mut sum_total: i64 = 0;
    for p in packs {
        let cycle_size = p.get("CycleCapacitySize").and_then(|v| v.as_i64()).unwrap_or(0);
        if cycle_size > 0 {
            let cycle_remain = p
                .get("CycleCapacityRemain")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .clamp(0, cycle_size);
            let cycle_used = p
                .get("CycleCapacityUsed")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let used = (cycle_size - cycle_remain).max(cycle_used);
            let remain = (cycle_size - used).max(0);
            sum_remain += remain;
            sum_total += cycle_size;
        } else {
            let cap_remain = p
                .get("CapacityRemain")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .max(0);
            let cap_size = p
                .get("CapacitySize")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .max(cap_remain);
            sum_remain += cap_remain;
            sum_total += cap_size;
        }
    }

    Ok(CreditBalance {
        remain: sum_remain,
        total: sum_total,
        unlimited: false,
    })
}

fn parse_enterprise_credits(raw: &str) -> Result<CreditBalance, String> {
    let root: Value =
        serde_json::from_str(raw).map_err(|e| format!("解析企业额度响应失败: {e}"))?;
    if let Some(code) = root.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = root.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            return Err(format!("查询企业额度失败 (code={code}): {msg}"));
        }
    }
    let data = root.get("data").unwrap_or(&root);
    let limit = data
        .get("limitNum")
        .or_else(|| data.get("limit_num"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if limit < 0 {
        return Ok(CreditBalance {
            remain: 0,
            total: -1,
            unlimited: true,
        });
    }
    let used = data
        .get("credit")
        .or_else(|| data.get("usedNum"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        .round() as i64;
    let remain = (limit - used).max(0);
    Ok(CreditBalance {
        remain,
        total: limit,
        unlimited: false,
    })
}

/// On-demand read-only credit balance query (no background polling).
pub async fn fetch_account_credits(
    client: &reqwest::Client,
    acc: &WorkBuddyAccount,
) -> Result<CreditBalance, String> {
    let base = chat_base_for(acc);

    if !acc.enterprise_id.trim().is_empty() {
        let endpoint = format!("{base}/v2/billing/meter/get-enterprise-user-usage");
        let resp = client
            .post(&endpoint)
            .headers(build_billing_headers(acc))
            .body("{}")
            .send()
            .await
            .map_err(|e| format!("查询企业额度失败: {e}"))?;
        let status = resp.status();
        let raw = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("查询企业额度 HTTP {status}"));
        }
        return parse_enterprise_credits(&raw);
    }

    let now = unix_now_secs();
    let req_body = serde_json::json!({
        "PageNumber": 1,
        "PageSize": 100,
        "ProductCode": "p_tcaca",
        "Status": [0, 3],
        "PackageEndTimeRangeBegin": format_cst_datetime(now),
        "PackageEndTimeRangeEnd": format_cst_datetime(now + 100 * 365 * 86_400),
    });

    let paths: &[&str] = if account_is_global(acc) {
        &[
            "/billing/meter/get-user-resource",
            "/v2/billing/meter/get-user-resource",
        ]
    } else {
        &["/v2/billing/meter/get-user-resource"]
    };

    let mut last_err = String::from("查询积分失败");
    for path in paths {
        let endpoint = format!("{base}{path}");
        match client
            .post(&endpoint)
            .headers(build_billing_headers(acc))
            .json(&req_body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let raw = resp.text().await.unwrap_or_default();
                if status == 404 {
                    last_err = format!("查询积分 HTTP 404 ({path})");
                    continue;
                }
                if status >= 400 {
                    return Err(format!("查询积分 HTTP {status}"));
                }
                return parse_personal_credits(&raw);
            }
            Err(e) => {
                last_err = format!("查询积分请求失败: {e}");
            }
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_personal_and_enterprise_credit_balances() {
        let personal_raw = r#"{
            "code": 0,
            "data": {
                "Response": {
                    "Data": {
                        "Accounts": [
                            { "CycleCapacitySize": 1000, "CycleCapacityRemain": 750, "CycleCapacityUsed": 250 },
                            { "CapacitySize": 500, "CapacityRemain": 320, "CapacityUsed": 180 }
                        ]
                    }
                }
            }
        }"#;
        let bal = parse_personal_credits(personal_raw).unwrap();
        assert_eq!(bal.remain, 1070);
        assert_eq!(bal.total, 1500);
        assert!(!bal.unlimited);

        let ent_raw = r#"{"code":0,"data":{"credit":812.45,"limitNum":2000}}"#;
        let ent = parse_enterprise_credits(ent_raw).unwrap();
        assert_eq!(ent.remain, 1188);
        assert_eq!(ent.total, 2000);
        assert!(!ent.unlimited);

        let unlim = parse_enterprise_credits(r#"{"code":0,"data":{"credit":10,"limitNum":-1}}"#).unwrap();
        assert!(unlim.unlimited);
    }

    #[test]
    fn classifies_daily_checkin_receipts_accurately() {
        let ok = classify_checkin_response(
            "u1",
            200,
            r#"{"credit":100,"streak_days":44,"is_streak_day":false}"#,
        );
        assert_eq!(ok.state, CheckinState::Checked);
        assert_eq!(ok.credit, Some(100));
        assert_eq!(ok.streak_days, Some(44));

        let already = classify_checkin_response(
            "u1",
            400,
            r#"{"code":10001,"msg":"今天已签到，请明天再来"}"#,
        );
        assert_eq!(already.state, CheckinState::Already);

        // Failure marker must win over "已签到"
        let fail = classify_checkin_response(
            "u1",
            200,
            r#"{"code":0,"msg":"签到失败，已签到天数不足 7 天"}"#,
        );
        assert_eq!(fail.state, CheckinState::Failed);

        let pending = classify_checkin_response(
            "u1",
            400,
            r#"{"code":10002,"msg":"请求处理中，请勿重复操作"}"#,
        );
        assert_eq!(pending.state, CheckinState::Pending);
    }
}
