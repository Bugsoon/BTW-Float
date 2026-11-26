//! Official WorkBuddy / CodeBuddy login site definitions, client fingerprint constants,
//! and session/trace ID generators (aligned with misakano7545/workbuddy2api-panel).

use crate::config::WorkBuddyAccount;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const CLIENT_VERSION_CN: &str = "5.11.3";
pub const CLIENT_VERSION_GLOBAL: &str = "5.9.4";
pub const CLI_VERSION_CN: &str = "2.176.0";
pub const CLI_VERSION_GLOBAL: &str = "2.165.0";

pub const CLIENT_IDENTITY_PLATFORM: &str = "windows";
pub const CLIENT_IDENTITY_PRODUCT: &str = "WorkBuddy";

pub const CHAT_BASE_CN: &str = "https://www.workbuddy.cn";
pub const BASE_INTL_OFFICIAL: &str = "https://www.codebuddy.ai";
pub const BASE_INTL_WORKBUDDY: &str = "https://www.workbuddy.ai";

pub const SITE_CN: &str = "cn";
pub const SITE_INTL: &str = "intl";
pub const SITE_WORKBUDDY: &str = "workbuddy";

pub const REALM_CN: &str = "cn";
pub const REALM_GLOBAL: &str = "global";

// Official OAuth device-flow business codes
pub const CODE_RETRY_FETCH_TOKEN: i64 = 11217;
pub const CODE_RETRY_FETCH_ACCOUNT: i64 = 12151;
pub const CODE_LICENSE_SEAT_LIMIT: i64 = 12005;
pub const CODE_LICENSE_EXPIRED: i64 = 11212;
pub const CODE_TRIAL_EXPIRED: i64 = 11216;

#[derive(Debug, Clone, Copy)]
pub struct LoginSite {
    pub id: &'static str,
    pub realm: &'static str,
    pub api: &'static str,
    pub platform: &'static str,
    pub user_agent: &'static str,
    pub domain: &'static str,
    pub version: &'static str,
}

pub const LOGIN_SITE_CN: LoginSite = LoginSite {
    id: SITE_CN,
    realm: REALM_CN,
    api: "https://copilot.tencent.com",
    platform: "workbuddy",
    user_agent: "WorkBuddy/5.11.3 WorkBuddy/5.11.3 CLI/2.176.0",
    domain: "www.workbuddy.cn",
    version: CLIENT_VERSION_CN,
};

pub const LOGIN_SITE_INTL: LoginSite = LoginSite {
    id: SITE_INTL,
    realm: REALM_GLOBAL,
    api: BASE_INTL_OFFICIAL,
    platform: "workbuddy-ai",
    user_agent: "workbuddy-ai/5.9.4 workbuddy-ai/5.9.4 CLI/2.165.0",
    domain: "www.codebuddy.ai",
    version: CLIENT_VERSION_GLOBAL,
};

pub const LOGIN_SITE_WORKBUDDY: LoginSite = LoginSite {
    id: SITE_WORKBUDDY,
    realm: REALM_GLOBAL,
    api: BASE_INTL_WORKBUDDY,
    platform: "workbuddy-ai",
    user_agent: "workbuddy-ai/5.9.4 workbuddy-ai/5.9.4 CLI/2.165.0",
    domain: "www.workbuddy.ai",
    version: CLIENT_VERSION_GLOBAL,
};

pub fn site_for(id: &str) -> LoginSite {
    match id.trim().to_ascii_lowercase().as_str() {
        SITE_INTL => LOGIN_SITE_INTL,
        SITE_WORKBUDDY => LOGIN_SITE_WORKBUDDY,
        _ => LOGIN_SITE_CN,
    }
}

impl LoginSite {
    pub fn state_url(self) -> String {
        format!("{}/v2/plugin/auth/state?platform={}", self.api, self.platform)
    }

    pub fn token_url(self, state: &str) -> String {
        format!("{}/v2/plugin/auth/token?state={}", self.api, state)
    }

    pub fn account_url(self, state: &str) -> String {
        format!("{}/v2/plugin/login/account?state={}", self.api, state)
    }

    pub fn decorate_auth_url(self, raw: &str, login_session_id: &str) -> String {
        let Ok(mut parsed) = url::Url::parse(raw) else {
            return raw.to_string();
        };
        let existing: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let has_key = |target: &str| existing.iter().any(|(k, _)| k == target);

        {
            let mut pairs = parsed.query_pairs_mut();
            if !self.version.is_empty() && !has_key("version") {
                pairs.append_pair("version", self.version);
            }
            if self.platform == "workbuddy"
                && !login_session_id.is_empty()
                && !has_key("loginSessionId")
            {
                pairs.append_pair("loginSessionId", login_session_id);
            }
        }
        parsed.to_string()
    }
}

pub fn is_login_pending(code: i64) -> bool {
    code == CODE_RETRY_FETCH_TOKEN || code == CODE_RETRY_FETCH_ACCOUNT
}

pub fn is_login_fatal(code: i64) -> bool {
    matches!(
        code,
        CODE_LICENSE_SEAT_LIMIT | CODE_LICENSE_EXPIRED | CODE_TRIAL_EXPIRED
    )
}

pub fn account_is_global(acc: &WorkBuddyAccount) -> bool {
    if acc.realm.eq_ignore_ascii_case(REALM_GLOBAL) {
        return true;
    }
    let d = acc.domain.trim().to_ascii_lowercase();
    d == "workbuddy.ai"
        || d.ends_with(".workbuddy.ai")
        || d == "codebuddy.ai"
        || d.ends_with(".codebuddy.ai")
}

pub fn base_for_domain(domain: &str) -> Option<&'static str> {
    let d = domain.trim().to_ascii_lowercase();
    if d == "codebuddy.ai" || d.ends_with(".codebuddy.ai") {
        Some(BASE_INTL_OFFICIAL)
    } else if d == "workbuddy.ai" || d.ends_with(".workbuddy.ai") {
        Some(BASE_INTL_WORKBUDDY)
    } else {
        None
    }
}

pub fn chat_base_for(acc: &WorkBuddyAccount) -> &'static str {
    if account_is_global(acc) {
        base_for_domain(&acc.domain).unwrap_or(BASE_INTL_WORKBUDDY)
    } else {
        CHAT_BASE_CN
    }
}

pub fn host_of(base: &str, fallback: &str) -> String {
    let trimmed = base
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base);
    let host = trimmed.split('/').next().unwrap_or("");
    if host.is_empty() {
        fallback.to_string()
    } else {
        host.to_string()
    }
}

pub fn client_version_for(acc: &WorkBuddyAccount) -> &'static str {
    if account_is_global(acc) {
        CLIENT_VERSION_GLOBAL
    } else {
        CLIENT_VERSION_CN
    }
}

pub fn cli_version_for(acc: &WorkBuddyAccount) -> &'static str {
    if account_is_global(acc) {
        CLI_VERSION_GLOBAL
    } else {
        CLI_VERSION_CN
    }
}

pub fn ua_brand_for(acc: &WorkBuddyAccount) -> &'static str {
    if account_is_global(acc) {
        "workbuddy-ai"
    } else {
        "WorkBuddy"
    }
}

/// Chat SDK User-Agent: `WorkBuddy/<ver> <brand>/<ver> CLI/<cli>`
pub fn chat_ua_for(acc: &WorkBuddyAccount) -> String {
    let ver = client_version_for(acc);
    let brand = ua_brand_for(acc);
    let cli = cli_version_for(acc);
    format!("WorkBuddy/{ver} {brand}/{ver} CLI/{cli}")
}

/// RPC / Refresh User-Agent: `<brand>/<ver> <brand>/<ver> CLI/<cli>`
pub fn rpc_ua_for(acc: &WorkBuddyAccount) -> String {
    let ver = client_version_for(acc);
    let brand = ua_brand_for(acc);
    let cli = cli_version_for(acc);
    format!("{brand}/{ver} {brand}/{ver} CLI/{cli}")
}

/// `/v3/config` probe User-Agent: `WorkBuddy/<ver> CLI/<cli>`
pub fn v3_probe_ua_for(acc: &WorkBuddyAccount) -> String {
    let ver = client_version_for(acc);
    let cli = cli_version_for(acc);
    format!("WorkBuddy/{ver} CLI/{cli}")
}

pub fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn unix_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Pure random 32-hex trace ID (matches official `X-Trace-ID` / `X-B3-TraceId`).
pub fn new_trace_id() -> String {
    hex::encode(Uuid::new_v4().as_bytes())
}

/// Pure random 16-hex span ID (matches official `X-B3-SpanId` / `X-B3-ParentSpanId`).
pub fn new_span_id() -> String {
    let bytes = Uuid::new_v4();
    hex::encode(&bytes.as_bytes()[..8])
}

/// Time-prefixed 32-hex message/request ID: 12-hex unix ms + 20-hex random.
pub fn new_message_id() -> String {
    let ms = unix_now_millis() & 0x0000_FFFF_FFFF_FFFF;
    let rand = Uuid::new_v4();
    format!("{:012x}{}", ms, hex::encode(&rand.as_bytes()[..10]))
}

/// Time-ordered UUIDv7 (36 chars with hyphens), matching real desktop `X-Conversation-ID` & `acp-connection-id`.
pub fn new_time_ordered_uuid_v7() -> String {
    let ms = unix_now_millis();
    let mut b = *Uuid::new_v4().as_bytes();
    b[0] = (ms >> 40) as u8;
    b[1] = (ms >> 32) as u8;
    b[2] = (ms >> 24) as u8;
    b[3] = (ms >> 16) as u8;
    b[4] = (ms >> 8) as u8;
    b[5] = ms as u8;
    b[6] = (b[6] & 0x0f) | 0x70;
    b[8] = (b[8] & 0x3f) | 0x80;
    format_uuid_bytes(&b)
}

/// Deterministic UUIDv4 derived from `(uid, conversation)` for `prompt_cache_key`.
pub fn build_cache_key_v4(uid: &str, conversation: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"wb2a:cache:");
    hasher.update(uid.as_bytes());
    hasher.update(b"|");
    hasher.update(conversation.as_bytes());
    let digest = hasher.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format_uuid_bytes(&b)
}

/// Deterministic UUIDv7-shaped session/ACP ID per `(uid, seed, salt)`.
pub fn build_time_ordered_uuid(uid: &str, seed: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"wb2a:v7:");
    hasher.update(uid.as_bytes());
    hasher.update(b"|");
    hasher.update(seed.as_bytes());
    hasher.update(b"|");
    hasher.update(salt.as_bytes());
    let digest = hasher.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x70;
    b[8] = (b[8] & 0x3f) | 0x80;
    format_uuid_bytes(&b)
}

fn format_uuid_bytes(b: &[u8; 16]) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        hex::encode(&b[0..4]),
        hex::encode(&b[4..6]),
        hex::encode(&b[6..8]),
        hex::encode(&b[8..10]),
        hex::encode(&b[10..16]),
    )
}

pub fn valid_uid(uid: &str) -> bool {
    !uid.is_empty()
        && uid.len() <= 64
        && uid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorates_cn_and_intl_auth_urls() {
        let cn = LOGIN_SITE_CN.decorate_auth_url(
            "https://www.workbuddy.cn/login?platform=workbuddy&state=abc",
            "sess-123",
        );
        assert!(cn.contains("version=5.11.3"));
        assert!(cn.contains("loginSessionId=sess-123"));

        let intl = LOGIN_SITE_INTL.decorate_auth_url(
            "https://www.codebuddy.ai/login/?platform=workbuddy-ai&state=xyz",
            "sess-123",
        );
        assert!(intl.contains("version=5.9.4"));
        assert!(!intl.contains("loginSessionId="));
    }

    #[test]
    fn id_formats_match_official_shapes() {
        let msg_id = new_message_id();
        assert_eq!(msg_id.len(), 32);
        let trace_id = new_trace_id();
        assert_eq!(trace_id.len(), 32);
        let span_id = new_span_id();
        assert_eq!(span_id.len(), 16);
        let v7 = new_time_ordered_uuid_v7();
        assert_eq!(v7.len(), 36);
        assert_eq!(v7.as_bytes()[14], b'7');
        let k1 = build_cache_key_v4("u1", "conv1");
        let k2 = build_cache_key_v4("u1", "conv1");
        let k3 = build_cache_key_v4("u2", "conv1");
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
        assert_eq!(k1.as_bytes()[14], b'4');
    }
}
