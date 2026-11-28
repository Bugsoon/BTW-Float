//! Embedded Single-Account WorkBuddy2API Kernel for BTW-Float.
//!
//! Pure-Rust minimal port of `misakano7545/workbuddy2api-panel` core:
//! - OAuth browser device-authorization login (CN / Intl / WorkBuddy)
//! - Singleflight automatic token refresh with DPAPI-encrypted persistence
//! - Human-like daily auto check-in (`08:15–10:45` CST randomized window + late-boot catch-up)
//! - Real-device WorkBuddy desktop headers & OpenAI payload normalization
//! - Minimal local OpenAI-compatible HTTP gateway (`127.0.0.1:<port>/v1/...`)

pub mod auth;
pub mod client;
pub mod protocol;
pub mod server;
pub mod site;

use crate::config::{save_config, ModelConfig, ProviderConfig, WorkBuddyAccount};
use crate::window_mgr::AppState;
use crate::workbuddy::auth::{LoginPollResult, LoginSessionStore, LoginStartResponse};
use crate::workbuddy::client::{
    build_upstream_http_client, daily_checkin, fetch_account_credits, fetch_account_models,
    refresh_account_token, send_chat_stream, CheckinReceipt, CheckinState, CreditBalance,
    UpstreamModel,
};
use crate::workbuddy::protocol::{ChatMeta, UpstreamErrKind, UpstreamError};
use crate::workbuddy::server::{start_kernel_server, RunningServer};
use crate::workbuddy::site::{account_is_global, unix_now_secs};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

pub const WORKBUDDY_PROVIDER_ID: &str = "workbuddy-local";
const TOKEN_REFRESH_LEAD_SECS: i64 = 1800; // Refresh proactively 30m before expiry
const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);
// Human-like daily check-in window in CST (UTC+8): 08:15:00 ~ 10:45:00 (9000 seconds span)
const HUMAN_CHECKIN_WINDOW_START_SECS: u32 = 8 * 3600 + 15 * 60;
const HUMAN_CHECKIN_WINDOW_SPAN_SECS: u32 = 150 * 60;
const CHECKIN_FAILURE_BACKOFF: Duration = Duration::from_secs(20 * 60);

#[derive(Debug, Clone, Default)]
struct AccountRuntimeState {
    last_error: Option<String>,
    last_err_kind: Option<UpstreamErrKind>,
    last_checkin_msg: Option<String>,
    next_checkin_retry: Option<Instant>,
    credits: Option<CreditBalance>,
    session_dead: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBuddyAccountView {
    pub uid: String,
    pub nickname: String,
    pub realm: String,
    pub site: String,
    pub domain: String,
    pub expires_at: i64,
    pub status: String, // "ready" | "no_credit" | "rate_limited" | "session_dead"
    pub last_error: Option<String>,
    pub checked_in_today: bool,
    pub last_checkin_date: String,
    pub last_checkin_msg: Option<String>,
    pub next_checkin_hint: Option<String>,
    pub credits_remain: Option<i64>,
    pub credits_total: Option<i64>,
    pub credits_unlimited: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBuddyStatusPayload {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub base_url: String,
    pub has_api_key: bool,
    pub auto_checkin: bool,
    pub last_error: Option<String>,
    pub account: Option<WorkBuddyAccountView>,
    pub cached_model_count: usize,
}

struct CachedModels {
    fetched_at: Instant,
    models: Vec<UpstreamModel>,
}

pub struct WorkBuddyKernel {
    app: Mutex<Option<AppHandle>>,
    account: Mutex<Option<WorkBuddyAccount>>,
    runtime: Mutex<AccountRuntimeState>,
    api_key: Mutex<String>,
    enabled: Mutex<bool>,
    auto_checkin: Mutex<bool>,
    port: Mutex<u16>,
    last_server_error: Mutex<Option<String>>,
    running_server: Mutex<Option<RunningServer>>,
    login_store: LoginSessionStore,
    model_cache: Mutex<Option<CachedModels>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    rpc_client: Mutex<Option<reqwest::Client>>,
    stream_client: Mutex<Option<reqwest::Client>>,
}

impl WorkBuddyKernel {
    pub fn new(
        enabled: bool,
        port: u16,
        api_key: String,
        auto_checkin: bool,
        account: Option<WorkBuddyAccount>,
    ) -> Self {
        Self {
            app: Mutex::new(None),
            account: Mutex::new(account),
            runtime: Mutex::new(AccountRuntimeState::default()),
            api_key: Mutex::new(api_key),
            enabled: Mutex::new(enabled),
            auto_checkin: Mutex::new(auto_checkin),
            port: Mutex::new(if port == 0 { 18080 } else { port }),
            last_server_error: Mutex::new(None),
            running_server: Mutex::new(None),
            login_store: LoginSessionStore::default(),
            model_cache: Mutex::new(None),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            rpc_client: Mutex::new(None),
            stream_client: Mutex::new(None),
        }
    }

    pub fn attach_app(&self, app: AppHandle) {
        *self.app.lock() = Some(app);
    }

    fn get_rpc_client(&self) -> Result<reqwest::Client, String> {
        let mut guard = self.rpc_client.lock();
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
        let c = build_upstream_http_client(30, false)?;
        *guard = Some(c.clone());
        Ok(c)
    }

    fn get_stream_client(&self) -> Result<reqwest::Client, String> {
        let mut guard = self.stream_client.lock();
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
        let c = build_upstream_http_client(120, true)?;
        *guard = Some(c.clone());
        Ok(c)
    }

    pub fn expected_api_key(&self) -> String {
        self.api_key.lock().trim().to_string()
    }

    pub fn current_base_url(&self) -> String {
        let port = *self.port.lock();
        format!("http://127.0.0.1:{port}/v1")
    }

    pub fn is_logged_in(&self) -> bool {
        self.account.lock().is_some()
    }

    pub fn status(&self) -> WorkBuddyStatusPayload {
        let enabled = *self.enabled.lock();
        let auto_checkin = *self.auto_checkin.lock();
        let port = *self.port.lock();
        let running = self.running_server.lock().is_some();
        let has_api_key = !self.api_key.lock().trim().is_empty();
        let last_error = self.last_server_error.lock().clone();
        let cached_model_count = self
            .model_cache
            .lock()
            .as_ref()
            .map(|c| c.models.len())
            .unwrap_or(0);

        let (today, now_cst_secs) = cst_date_and_secs_from_unix(unix_now_secs());
        let acc_opt = self.account.lock().clone();
        let rt = self.runtime.lock().clone();

        let account = acc_opt.map(|a| {
            let zero_credit = rt
                .credits
                .is_some_and(|c| !c.unlimited && c.total > 0 && c.remain <= 0);
            let status = if rt.session_dead {
                "session_dead"
            } else if rt.last_err_kind == Some(UpstreamErrKind::HardCredit) || zero_credit {
                "no_credit"
            } else if rt.last_err_kind == Some(UpstreamErrKind::SoftRate) {
                "rate_limited"
            } else {
                "ready"
            };
            let checked_in_today = a.last_checkin_date == today;
            let eligible_checkin =
                auto_checkin && !account_is_global(&a) && a.enterprise_id.trim().is_empty();
            let next_checkin_hint = if !eligible_checkin || checked_in_today {
                None
            } else {
                let target = daily_human_target_secs(&a.uid, &today);
                if now_cst_secs < target {
                    Some(format!("计划今日 {} 自动签", format_hhmm(target)))
                } else {
                    Some("已过今日时段，即将自动补签".to_string())
                }
            };
            WorkBuddyAccountView {
                uid: a.uid,
                nickname: a.nickname,
                realm: a.realm,
                site: a.site,
                domain: a.domain,
                expires_at: a.expires_at,
                status: status.to_string(),
                last_error: rt.last_error,
                checked_in_today,
                last_checkin_date: a.last_checkin_date,
                last_checkin_msg: rt.last_checkin_msg,
                next_checkin_hint,
                credits_remain: rt.credits.map(|c| c.remain),
                credits_total: rt.credits.map(|c| c.total),
                credits_unlimited: rt.credits.is_some_and(|c| c.unlimited),
            }
        });

        WorkBuddyStatusPayload {
            enabled,
            running,
            port,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            has_api_key,
            auto_checkin,
            last_error,
            account,
            cached_model_count,
        }
    }

    pub async fn sync_server_lifecycle(self: &Arc<Self>) -> Result<(), String> {
        let want_enabled = *self.enabled.lock();
        let want_port = *self.port.lock();

        if !want_enabled {
            let old = self.running_server.lock().take();
            if let Some(srv) = old {
                srv.stop();
            }
            *self.last_server_error.lock() = None;
            return Ok(());
        }

        let already_on_port = self
            .running_server
            .lock()
            .as_ref()
            .is_some_and(|s| s.port == want_port);
        if already_on_port {
            *self.last_server_error.lock() = None;
            return Ok(());
        }

        let old = self.running_server.lock().take();
        if let Some(old) = old {
            old.stop();
            tokio::time::sleep(Duration::from_millis(60)).await;
        }

        match start_kernel_server(Arc::clone(self), want_port).await {
            Ok(srv) => {
                *self.running_server.lock() = Some(srv);
                *self.last_server_error.lock() = None;
                Ok(())
            }
            Err(e) => {
                *self.last_server_error.lock() = Some(e.clone());
                Err(e)
            }
        }
    }

    pub async fn apply_settings(
        self: &Arc<Self>,
        enabled: bool,
        port: u16,
        api_key: String,
    ) -> Result<WorkBuddyStatusPayload, String> {
        let valid_port = if port < 1024 { 18080 } else { port };
        *self.enabled.lock() = enabled;
        *self.port.lock() = valid_port;
        *self.api_key.lock() = api_key.trim().to_string();

        self.persist_workbuddy_to_config(true)?;
        let _ = self.sync_server_lifecycle().await;
        Ok(self.status())
    }

    pub async fn start_login(&self, site_id: &str) -> Result<LoginStartResponse, String> {
        self.login_store.start_login(site_id).await
    }

    pub fn cancel_login(&self, state: &str) {
        self.login_store.cancel(state);
    }

    pub async fn poll_login(self: &Arc<Self>, state: &str) -> Result<LoginPollResult, String> {
        let (res, maybe_acc) = self.login_store.poll_login(state).await?;
        if let Some(acc) = maybe_acc {
            *self.account.lock() = Some(acc);
            *self.runtime.lock() = AccountRuntimeState::default();
            *self.enabled.lock() = true;
            *self.model_cache.lock() = None;
            self.persist_workbuddy_to_config(true)?;
            let _ = self.sync_server_lifecycle().await;
            let _ = self.sync_models_to_provider(false).await;

            // Best-effort immediate daily check-in for newly logged-in CN personal account
            if *self.auto_checkin.lock() {
                let kernel = Arc::clone(self);
                tauri::async_runtime::spawn(async move {
                    let _ = kernel.checkin_now().await;
                });
            }
        }
        Ok(res)
    }

    pub fn logout(&self) -> Result<WorkBuddyStatusPayload, String> {
        *self.account.lock() = None;
        *self.runtime.lock() = AccountRuntimeState::default();
        *self.model_cache.lock() = None;
        self.persist_workbuddy_to_config(false)?;
        Ok(self.status())
    }

    pub fn set_auto_checkin(&self, enabled: bool) -> Result<WorkBuddyStatusPayload, String> {
        *self.auto_checkin.lock() = enabled;
        self.persist_workbuddy_to_config(false)?;
        Ok(self.status())
    }

    /// Ensures the logged-in account has a fresh access token (singleflight deduplicated).
    pub async fn ensure_fresh_account(&self, force: bool) -> Result<WorkBuddyAccount, String> {
        let snap = self
            .account
            .lock()
            .clone()
            .ok_or_else(|| "尚未登录 WorkBuddy 账号，请先在设置中登录".to_string())?;

        let now = unix_now_secs();
        let needs_refresh =
            force || (snap.expires_at > 0 && snap.expires_at - now <= TOKEN_REFRESH_LEAD_SECS);
        if !needs_refresh || snap.refresh_token.trim().is_empty() {
            return Ok(snap);
        }

        let _guard = self.refresh_lock.lock().await;

        let current = self
            .account
            .lock()
            .clone()
            .ok_or_else(|| "尚未登录 WorkBuddy 账号".to_string())?;
        if !force
            && current.expires_at > 0
            && current.expires_at - unix_now_secs() > TOKEN_REFRESH_LEAD_SECS
        {
            return Ok(current);
        }

        let client = self.get_rpc_client()?;
        let refreshed = refresh_account_token(&client, &current).await?;

        let updated = {
            let mut slot = self.account.lock();
            let acc = slot
                .as_mut()
                .ok_or_else(|| "账号已退出登录".to_string())?;
            acc.access_token = refreshed.access_token;
            if let Some(rt) = refreshed.refresh_token {
                acc.refresh_token = rt;
            }
            if let Some(d) = refreshed.domain {
                acc.domain = d;
            }
            if let Some(exp) = refreshed.expires_at {
                acc.expires_at = exp;
            }
            acc.clone()
        };

        {
            let mut rt = self.runtime.lock();
            rt.session_dead = false;
            rt.last_error = None;
        }

        let _ = self.persist_workbuddy_to_config(false);
        Ok(updated)
    }

    /// Executes daily check-in right now for the logged-in account.
    pub async fn checkin_now(&self) -> Result<CheckinReceipt, String> {
        let acc = self.ensure_fresh_account(false).await?;
        let client = self.get_rpc_client()?;
        let receipt = daily_checkin(&client, &acc).await?;

        let today = today_cst_date();
        let should_mark_done = matches!(receipt.state, CheckinState::Checked | CheckinState::Already);

        {
            let mut rt = self.runtime.lock();
            rt.last_checkin_msg = Some(receipt.message.clone());
            if should_mark_done {
                rt.next_checkin_retry = None;
            } else if matches!(receipt.state, CheckinState::Failed | CheckinState::Pending) {
                rt.next_checkin_retry = Some(Instant::now() + CHECKIN_FAILURE_BACKOFF);
            }
        }

        if should_mark_done {
            let mut changed = false;
            {
                if let Some(a) = self.account.lock().as_mut() {
                    if a.last_checkin_date != today {
                        a.last_checkin_date = today;
                        changed = true;
                    }
                }
            }
            if changed {
                let _ = self.persist_workbuddy_to_config(false);
            }
            // Refresh balance right after check-in so UI reflects newly awarded credits
            if let Ok(bal) = fetch_account_credits(&client, &acc).await {
                let mut rt = self.runtime.lock();
                rt.credits = Some(bal);
                if bal.unlimited || bal.remain > 0 {
                    if rt.last_err_kind == Some(UpstreamErrKind::HardCredit) {
                        rt.last_err_kind = None;
                        rt.last_error = None;
                    }
                }
            }
        }

        Ok(receipt)
    }

    /// On-demand credit balance refresh (also refreshes token if expiring soon or session_dead).
    pub async fn refresh_account_status(&self) -> Result<WorkBuddyStatusPayload, String> {
        let force_token = self.runtime.lock().session_dead;
        let acc = self.ensure_fresh_account(force_token).await?;
        let client = self.get_rpc_client()?;
        let bal = fetch_account_credits(&client, &acc).await?;
        {
            let mut rt = self.runtime.lock();
            rt.credits = Some(bal);
            if bal.unlimited || bal.remain > 0 {
                if rt.last_err_kind == Some(UpstreamErrKind::HardCredit) {
                    rt.last_err_kind = None;
                    rt.last_error = None;
                }
            }
        }
        Ok(self.status())
    }

    /// Human-like auto check-in tick: runs only if logged in as a CN personal account,
    /// not yet checked in today (CST), past today's randomized slot (`08:15–10:45`), and not in backoff.
    async fn checkin_if_due(&self) {
        let (today, now_cst_secs) = cst_date_and_secs_from_unix(unix_now_secs());
        let now = Instant::now();
        let due = {
            let acc_opt = self.account.lock();
            let rt = self.runtime.lock();
            match acc_opt.as_ref() {
                Some(a)
                    if !account_is_global(a)
                        && a.enterprise_id.trim().is_empty()
                        && a.last_checkin_date != today
                        && rt.next_checkin_retry.is_none_or(|until| until <= now) =>
                {
                    now_cst_secs >= daily_human_target_secs(&a.uid, &today)
                }
                _ => false,
            }
        };

        if !due {
            return;
        }

        if let Err(e) = self.checkin_now().await {
            let mut rt = self.runtime.lock();
            rt.last_checkin_msg = Some(e);
            rt.next_checkin_retry = Some(Instant::now() + CHECKIN_FAILURE_BACKOFF);
        }
    }

    /// Human-like background check-in loop:
    /// - Waits a natural random 15–75s after startup;
    /// - Checks in-memory every 2 minutes whether today's randomized CST morning slot (`08:15–10:45`)
    ///   has arrived (never checks in during `00:00–08:15`, catches up naturally on late PC boot).
    pub fn start_auto_checkin_loop(self: &Arc<Self>) {
        let kernel = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let startup_jitter = random_range_u64(15, 75);
            tokio::time::sleep(Duration::from_secs(startup_jitter)).await;
            loop {
                if *kernel.auto_checkin.lock() {
                    kernel.checkin_if_due().await;
                }
                tokio::time::sleep(Duration::from_secs(120)).await;
            }
        });
    }

    pub async fn send_chat(
        &self,
        acc: &WorkBuddyAccount,
        prepared_body: &Value,
        meta: &ChatMeta,
    ) -> Result<reqwest::Response, UpstreamError> {
        let client = self.get_stream_client().map_err(|e| UpstreamError {
            kind: UpstreamErrKind::Server,
            status: 500,
            msg: e,
        })?;
        send_chat_stream(&client, acc, prepared_body, meta).await
    }

    pub fn mark_chat_success(&self) {
        let mut rt = self.runtime.lock();
        rt.session_dead = false;
        rt.last_error = None;
        rt.last_err_kind = None;
    }

    pub fn record_chat_error(&self, kind: UpstreamErrKind) {
        let mut rt = self.runtime.lock();
        rt.last_error = Some(kind.as_str().to_string());
        rt.last_err_kind = Some(kind);
        if kind == UpstreamErrKind::SessionDead {
            rt.session_dead = true;
        }
    }

    pub async fn get_cached_or_fetch_models(
        &self,
        force_refresh: bool,
    ) -> Result<Vec<UpstreamModel>, String> {
        if !force_refresh {
            if let Some(cached) = self.model_cache.lock().as_ref() {
                if cached.fetched_at.elapsed() < MODEL_CACHE_TTL && !cached.models.is_empty() {
                    return Ok(cached.models.clone());
                }
            }
        }

        let acc = self.ensure_fresh_account(false).await?;
        let client = self.get_rpc_client()?;
        let models = fetch_account_models(&client, &acc).await?;
        if models.is_empty() {
            return Err("上游返回了空的模型列表".to_string());
        }
        *self.model_cache.lock() = Some(CachedModels {
            fetched_at: Instant::now(),
            models: models.clone(),
        });
        Ok(models)
    }

    /// Syncs discovered WorkBuddy models into BTW-Float's built-in `workbuddy-local` provider.
    pub async fn sync_models_to_provider(
        &self,
        set_as_default: bool,
    ) -> Result<Vec<ModelConfig>, String> {
        let upstream_models = self.get_cached_or_fetch_models(true).await?;
        let mapped: Vec<ModelConfig> = upstream_models
            .into_iter()
            .map(|m| ModelConfig {
                id: m.id,
                label: m.name,
                vision: m.vision,
                tool_calling: m.tool_calling,
            })
            .collect();

        let base_url = self.current_base_url();
        let api_key = self.expected_api_key();

        let app_opt = self.app.lock().clone();
        if let Some(app) = app_opt {
            let state = app.state::<AppState>();
            let updated_cfg = {
                let mut cfg = state.config.lock();
                if let Some(prov) = cfg
                    .providers
                    .iter_mut()
                    .find(|p| p.id == WORKBUDDY_PROVIDER_ID)
                {
                    prov.base_url = base_url;
                    prov.api_key = api_key;
                    prov.models = mapped.clone();
                } else {
                    cfg.providers.insert(
                        0,
                        ProviderConfig {
                            id: WORKBUDDY_PROVIDER_ID.to_string(),
                            name: "WorkBuddy 内核".to_string(),
                            base_url,
                            api_key,
                            models: mapped.clone(),
                        },
                    );
                }

                let active_has_no_key = cfg
                    .providers
                    .iter()
                    .find(|p| p.id == cfg.active_provider_id)
                    .is_some_and(|p| {
                        p.api_key.trim().is_empty()
                            && !p.base_url.contains("127.0.0.1")
                            && !p.base_url.contains("localhost")
                    });

                let preferred_default = mapped
                    .iter()
                    .find(|m| m.id.eq_ignore_ascii_case("deepseek-v4.1-flash"))
                    .or_else(|| mapped.first())
                    .map(|m| m.id.clone());

                let already_valid_wb_model = cfg.active_provider_id == WORKBUDDY_PROVIDER_ID
                    && mapped.iter().any(|m| m.id == cfg.active_model_id);

                if (set_as_default || active_has_no_key) && !already_valid_wb_model {
                    if let Some(target_model) = preferred_default {
                        cfg.active_provider_id = WORKBUDDY_PROVIDER_ID.to_string();
                        cfg.active_model_id = target_model;
                    }
                }
                save_config(&cfg)?;
                crate::config::redact_workbuddy_secrets(cfg.clone())
            };
            let _ = app.emit("config:updated", &updated_cfg);
        }

        Ok(mapped)
    }

    fn persist_workbuddy_to_config(&self, sync_provider_endpoint: bool) -> Result<(), String> {
        let app_opt = self.app.lock().clone();
        let Some(app) = app_opt else {
            return Ok(());
        };

        let enabled = *self.enabled.lock();
        let auto_checkin = *self.auto_checkin.lock();
        let port = *self.port.lock();
        let api_key = self.api_key.lock().clone();
        let account = self.account.lock().clone();
        let base_url = format!("http://127.0.0.1:{port}/v1");

        let state = app.state::<AppState>();
        let updated_cfg = {
            let mut cfg = state.config.lock();
            cfg.workbuddy.enabled = enabled;
            cfg.workbuddy.auto_checkin = auto_checkin;
            cfg.workbuddy.port = port;
            cfg.workbuddy.api_key = api_key.clone();
            cfg.workbuddy.account = account;
            cfg.workbuddy.accounts.clear();

            if sync_provider_endpoint {
                if let Some(prov) = cfg
                    .providers
                    .iter_mut()
                    .find(|p| p.id == WORKBUDDY_PROVIDER_ID)
                {
                    prov.base_url = base_url;
                    prov.api_key = api_key;
                }
            }
            save_config(&cfg)?;
            crate::config::redact_workbuddy_secrets(cfg.clone())
        };
        let _ = app.emit("config:updated", &updated_cfg);
        Ok(())
    }
}

/// Returns today's calendar date in China Standard Time (UTC+8) as `YYYY-MM-DD`.
pub fn today_cst_date() -> String {
    cst_date_and_secs_from_unix(unix_now_secs()).0
}

/// Returns `(YYYY-MM-DD, seconds_since_midnight)` in China Standard Time (UTC+8).
fn cst_date_and_secs_from_unix(unix_secs: i64) -> (String, u32) {
    let cst_secs = unix_secs + 8 * 3600;
    let days = cst_secs.div_euclid(86_400);
    let secs_of_day = cst_secs.rem_euclid(86_400) as u32;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (format!("{year:04}-{m:02}-{d:02}"), secs_of_day)
}

/// Deterministic human-like daily check-in target second in `[08:15:00, 10:45:00)` CST.
fn daily_human_target_secs(uid: &str, date: &str) -> u32 {
    let mut hasher = Sha256::new();
    hasher.update(b"wb2a:human-checkin:");
    hasher.update(uid.as_bytes());
    hasher.update(b"|");
    hasher.update(date.as_bytes());
    let digest = hasher.finalize();
    let raw = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    HUMAN_CHECKIN_WINDOW_START_SECS + (raw % HUMAN_CHECKIN_WINDOW_SPAN_SECS)
}

fn format_hhmm(secs_of_day: u32) -> String {
    let h = (secs_of_day / 3600) % 24;
    let m = (secs_of_day % 3600) / 60;
    format!("{h:02}:{m:02}")
}

fn random_range_u64(min_inclusive: u64, max_inclusive: u64) -> u64 {
    if max_inclusive <= min_inclusive {
        return min_inclusive;
    }
    let span = max_inclusive - min_inclusive + 1;
    let bytes = *Uuid::new_v4().as_bytes();
    let v = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    min_inclusive + (v % span)
}

#[cfg(test)]
mod tests {
    use super::{
        cst_date_and_secs_from_unix, daily_human_target_secs, format_hhmm,
        HUMAN_CHECKIN_WINDOW_SPAN_SECS, HUMAN_CHECKIN_WINDOW_START_SECS,
    };

    #[test]
    fn formats_cst_calendar_dates_across_midnight_and_leap_years() {
        assert_eq!(
            cst_date_and_secs_from_unix(1_709_135_999),
            ("2024-02-28".to_string(), 86_399)
        );
        assert_eq!(
            cst_date_and_secs_from_unix(1_709_136_000),
            ("2024-02-29".to_string(), 0)
        );
        assert_eq!(
            cst_date_and_secs_from_unix(1_795_701_600),
            ("2026-11-26".to_string(), 22 * 3600)
        );
    }

    #[test]
    fn human_checkin_slots_stay_inside_window_and_vary_by_day_and_uid() {
        let t1 = daily_human_target_secs("uid-a", "2026-11-26");
        let t2 = daily_human_target_secs("uid-a", "2026-11-27");
        let t3 = daily_human_target_secs("uid-b", "2026-11-26");
        let end = HUMAN_CHECKIN_WINDOW_START_SECS + HUMAN_CHECKIN_WINDOW_SPAN_SECS;

        for t in [t1, t2, t3] {
            assert!(t >= HUMAN_CHECKIN_WINDOW_START_SECS && t < end);
        }
        assert_eq!(t1, daily_human_target_secs("uid-a", "2026-11-26"));
        assert_ne!(t1, t2);
        assert_ne!(t1, t3);
        assert_eq!(format_hhmm(8 * 3600 + 15 * 60), "08:15");
    }
}
