//! WorkBuddy OAuth device-authorization flow (in-process port of misakano7545/workbuddy2api-panel):
//! 1. `POST /v2/plugin/auth/state?platform=...` -> `{ state, authUrl }`
//! 2. Open `authUrl` in user's browser
//! 3. Poll `GET /v2/plugin/auth/token?state=...`
//! 4. Fetch `GET /v2/plugin/login/account?state=...` -> `{ uid, nickname, enterpriseId }`

use crate::config::WorkBuddyAccount;
use crate::workbuddy::client::build_upstream_http_client;
use crate::workbuddy::site::{
    is_login_fatal, is_login_pending, new_trace_id, site_for, unix_now_secs, valid_uid, LoginSite,
};
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use uuid::Uuid;

const LOGIN_SESSION_TTL: Duration = Duration::from_secs(600);
const LOGIN_TRANSPORT_ATTEMPTS: usize = 3;

#[derive(Debug, Clone)]
struct PendingLogin {
    site_id: String,
    created_at: Instant,
    processing: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStartResponse {
    pub state: String,
    pub auth_url: String,
    pub site: String,
    pub realm: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPollResult {
    /// "pending" | "done" | "fatal" | "expired"
    pub status: String,
    pub message: Option<String>,
    pub uid: Option<String>,
    pub nickname: Option<String>,
    pub realm: Option<String>,
}

#[derive(Debug)]
enum LoginJsonError {
    Envelope { code: i64, msg: String },
    Http { status: u16, msg: String },
    Transport(String),
}

impl std::fmt::Display for LoginJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Envelope { code, msg } => write!(f, "code={code} msg={msg}"),
            Self::Http { status, msg } => write!(f, "HTTP {status}: {msg}"),
            Self::Transport(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct LoginEnvelope {
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Value,
}

#[derive(Default)]
pub struct LoginSessionStore {
    sessions: Mutex<HashMap<String, PendingLogin>>,
}

impl LoginSessionStore {
    pub fn cancel(&self, state: &str) {
        self.sessions.lock().remove(state);
    }

    pub async fn start_login(&self, site_id: &str) -> Result<LoginStartResponse, String> {
        let site = site_for(site_id);
        let client = build_upstream_http_client(30, false)?;
        let url = site.state_url();
        let headers = build_state_headers(site);

        let data = do_login_json(&client, reqwest::Method::POST, &url, headers, Some("{}"))
            .await
            .map_err(|e| format!("获取授权链接失败: {e}"))?;

        let state = data
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let raw_auth_url = data
            .get("authUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if state.is_empty() || raw_auth_url.is_empty() {
            return Err("上游未返回有效的 state 或 authUrl".to_string());
        }

        let login_session_id = Uuid::new_v4().to_string();
        let auth_url = site.decorate_auth_url(&raw_auth_url, &login_session_id);

        {
            let mut guard = self.sessions.lock();
            guard.retain(|_, s| s.created_at.elapsed() < LOGIN_SESSION_TTL);
            guard.insert(
                state.clone(),
                PendingLogin {
                    site_id: site.id.to_string(),
                    created_at: Instant::now(),
                    processing: false,
                },
            );
        }

        Ok(LoginStartResponse {
            state,
            auth_url,
            site: site.id.to_string(),
            realm: site.realm.to_string(),
        })
    }

    /// Polls once. Returns `(LoginPollResult, Option<WorkBuddyAccount>)` where the account
    /// is populated only when `status == "done"`.
    pub async fn poll_login(
        &self,
        state: &str,
    ) -> Result<(LoginPollResult, Option<WorkBuddyAccount>), String> {
        let site_id = {
            let mut guard = self.sessions.lock();
            let Some(sess) = guard.get_mut(state) else {
                return Ok((
                    LoginPollResult {
                        status: "expired".to_string(),
                        message: Some("登录会话已过期，请重新点击登录".to_string()),
                        uid: None,
                        nickname: None,
                        realm: None,
                    },
                    None,
                ));
            };
            if sess.created_at.elapsed() >= LOGIN_SESSION_TTL {
                guard.remove(state);
                return Ok((
                    LoginPollResult {
                        status: "expired".to_string(),
                        message: Some("登录超时，请重新点击登录".to_string()),
                        uid: None,
                        nickname: None,
                        realm: None,
                    },
                    None,
                ));
            }
            if sess.processing {
                return Ok((
                    LoginPollResult {
                        status: "pending".to_string(),
                        message: Some("正在校验登录状态...".to_string()),
                        uid: None,
                        nickname: None,
                        realm: None,
                    },
                    None,
                ));
            }
            sess.processing = true;
            sess.site_id.clone()
        };

        let result = self.poll_login_inner(state, &site_id).await;

        // Release processing gate unless session was already removed on done/fatal
        {
            let mut guard = self.sessions.lock();
            if let Some(sess) = guard.get_mut(state) {
                sess.processing = false;
            }
        }

        result
    }

    async fn poll_login_inner(
        &self,
        state: &str,
        site_id: &str,
    ) -> Result<(LoginPollResult, Option<WorkBuddyAccount>), String> {
        let site = site_for(site_id);
        let client = build_upstream_http_client(30, false)?;

        // Step 1: Poll auth/token
        let tok_res = do_login_json(
            &client,
            reqwest::Method::GET,
            &site.token_url(state),
            build_token_headers(site),
            None,
        )
        .await;

        let tok_data = match tok_res {
            Ok(data) => data,
            Err(LoginJsonError::Envelope { code, msg }) => {
                if is_login_fatal(code) {
                    self.sessions.lock().remove(state);
                    return Ok((
                        LoginPollResult {
                            status: "fatal".to_string(),
                            message: Some(format!("登录被上游拒绝 (code={code}): {msg}")),
                            uid: None,
                            nickname: None,
                            realm: None,
                        },
                        None,
                    ));
                }
                let hint = if is_login_pending(code) {
                    "等待浏览器完成授权..."
                } else {
                    "等待登录确认..."
                };
                return Ok((
                    LoginPollResult {
                        status: "pending".to_string(),
                        message: Some(hint.to_string()),
                        uid: None,
                        nickname: None,
                        realm: None,
                    },
                    None,
                ));
            }
            Err(e) => {
                return Ok((
                    LoginPollResult {
                        status: "pending".to_string(),
                        message: Some(format!("轮询等待中 ({e})")),
                        uid: None,
                        nickname: None,
                        realm: None,
                    },
                    None,
                ));
            }
        };

        let access_token = tok_data
            .get("accessToken")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if access_token.is_empty() {
            return Ok((
                LoginPollResult {
                    status: "pending".to_string(),
                    message: Some("等待浏览器完成授权...".to_string()),
                    uid: None,
                    nickname: None,
                    realm: None,
                },
                None,
            ));
        }
        let refresh_token = tok_data
            .get("refreshToken")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let expires_in = tok_data
            .get("expiresIn")
            .and_then(|v| v.as_i64())
            .unwrap_or(5_184_000);
        let domain = tok_data
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or(site.domain)
            .trim()
            .to_string();

        // Step 2: Fetch account info
        let acct_res = do_login_json(
            &client,
            reqwest::Method::GET,
            &site.account_url(state),
            build_account_headers(site, &access_token),
            None,
        )
        .await;

        let acct_data = match acct_res {
            Ok(data) => data,
            Err(LoginJsonError::Envelope { code, msg }) => {
                if is_login_pending(code) {
                    return Ok((
                        LoginPollResult {
                            status: "pending".to_string(),
                            message: Some("正在同步账号信息...".to_string()),
                            uid: None,
                            nickname: None,
                            realm: None,
                        },
                        None,
                    ));
                }
                if is_login_fatal(code) {
                    self.sessions.lock().remove(state);
                    return Ok((
                        LoginPollResult {
                            status: "fatal".to_string(),
                            message: Some(format!("账号校验被拒 (code={code}): {msg}")),
                            uid: None,
                            nickname: None,
                            realm: None,
                        },
                        None,
                    ));
                }
                return Err(format!("获取账号信息失败 (code={code}): {msg}"));
            }
            Err(e) => return Err(format!("获取账号信息失败: {e}")),
        };

        let uid = acct_data
            .get("uid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if !valid_uid(&uid) {
            return Err("上游返回了无效的 uid，请重试登录".to_string());
        }

        let nickname = acct_data
            .get("nickname")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let enterprise_id = acct_data
            .get("enterpriseId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let account = WorkBuddyAccount {
            uid: uid.clone(),
            nickname: nickname.clone(),
            realm: site.realm.to_string(),
            site: site.id.to_string(),
            domain,
            enterprise_id,
            access_token,
            refresh_token,
            expires_at: unix_now_secs() + expires_in.max(3600),
            device_token: String::new(),
            last_checkin_date: String::new(),
        };

        self.sessions.lock().remove(state);

        Ok((
            LoginPollResult {
                status: "done".to_string(),
                message: Some("登录成功".to_string()),
                uid: Some(uid),
                nickname: Some(nickname),
                realm: Some(site.realm.to_string()),
            },
            Some(account),
        ))
    }
}

fn set_hdr(headers: &mut HeaderMap, k: &'static str, v: &str) {
    if v.is_empty() {
        return;
    }
    if let (Ok(name), Ok(val)) = (
        HeaderName::from_bytes(k.as_bytes()),
        HeaderValue::from_str(v),
    ) {
        headers.insert(name, val);
    }
}

fn base_login_headers(site: LoginSite) -> HeaderMap {
    let mut h = HeaderMap::new();
    set_hdr(&mut h, "accept", "application/json, text/plain, */*");
    set_hdr(&mut h, "user-agent", site.user_agent);
    set_hdr(&mut h, "x-product", "SaaS");
    set_hdr(&mut h, "x-request-id", &new_trace_id());
    h
}

fn build_state_headers(site: LoginSite) -> HeaderMap {
    let mut h = base_login_headers(site);
    set_hdr(&mut h, "content-type", "application/json");
    set_hdr(&mut h, "x-domain", site.domain);
    set_hdr(&mut h, "x-custom-channel-id", "unknown");
    set_hdr(&mut h, "x-no-authorization", "true");
    set_hdr(&mut h, "x-no-user-id", "true");
    set_hdr(&mut h, "x-no-enterprise-id", "true");
    set_hdr(&mut h, "x-no-department-info", "true");
    h
}

fn build_token_headers(site: LoginSite) -> HeaderMap {
    let mut h = base_login_headers(site);
    set_hdr(&mut h, "x-no-authorization", "true");
    set_hdr(&mut h, "x-no-user-id", "true");
    set_hdr(&mut h, "x-no-enterprise-id", "true");
    set_hdr(&mut h, "x-no-department-info", "true");
    h
}

fn build_account_headers(site: LoginSite, access_token: &str) -> HeaderMap {
    let mut h = base_login_headers(site);
    set_hdr(&mut h, "x-domain", site.domain);
    set_hdr(&mut h, "x-no-user-id", "true");
    set_hdr(&mut h, "x-no-enterprise-id", "true");
    set_hdr(&mut h, "x-no-department-info", "true");
    set_hdr(
        &mut h,
        "authorization",
        &format!("Bearer {access_token}"),
    );
    h
}

async fn do_login_json(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    headers: HeaderMap,
    body: Option<&str>,
) -> Result<Value, LoginJsonError> {
    let mut last_err = LoginJsonError::Transport("unknown error".to_string());

    for attempt in 0..LOGIN_TRANSPORT_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis((attempt as u64) * 400)).await;
        }
        let mut req = client.request(method.clone(), url).headers(headers.clone());
        if let Some(b) = body {
            req = req.body(b.to_string());
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = LoginJsonError::Transport(e.to_string());
                continue;
            }
        };
        let status = resp.status().as_u16();
        let raw = resp.text().await.unwrap_or_default();
        if status >= 300 {
            let snippet: String = raw.chars().take(160).collect();
            return Err(LoginJsonError::Http {
                status,
                msg: snippet,
            });
        }
        let env: LoginEnvelope = serde_json::from_str(&raw).map_err(|e| LoginJsonError::Http {
            status,
            msg: format!("parse json: {e}"),
        })?;
        if env.code != 0 {
            return Err(LoginJsonError::Envelope {
                code: env.code,
                msg: env.msg,
            });
        }
        return Ok(env.data);
    }

    Err(last_err)
}
