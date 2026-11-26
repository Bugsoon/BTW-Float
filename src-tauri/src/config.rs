use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vision: bool,
    #[serde(default = "default_true")]
    pub tool_calling: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyConfig {
    pub toggle_popup: String,
    #[serde(default = "default_mouse_toggle")]
    pub mouse_toggle: String,
    pub capture_region: String,
    pub capture_window: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MousePriorityConfig {
    #[serde(default = "default_true")]
    pub passthrough_fullscreen: bool,
    #[serde(default = "default_blacklist")]
    pub blacklist_processes: Vec<String>,
    #[serde(default = "default_blacklist")]
    pub disabled_processes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiConfig {
    #[serde(default = "default_true")]
    pub show_ball: bool,
    pub ball_x: Option<i32>,
    pub ball_y: Option<i32>,
    #[serde(default = "default_true")]
    pub ball_auto_hide_edge: bool,
    #[serde(default = "default_true")]
    pub popup_auto_hide_on_blur: bool,
    #[serde(default = "default_freshness")]
    pub clipboard_image_freshness_secs: u64,
    #[serde(default = "default_true")]
    pub auto_submit_selection: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub default_web_search: bool,
    #[serde(default)]
    pub show_search_sources: bool,
    #[serde(default = "default_quick_prompts")]
    pub quick_prompts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBuddyAccount {
    pub uid: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default = "default_wb_realm")]
    pub realm: String, // "cn" | "global"
    #[serde(default = "default_wb_site")]
    pub site: String, // "cn" | "intl" | "workbuddy"
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub enterprise_id: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default)]
    pub device_token: String,
    #[serde(default)]
    pub last_checkin_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBuddyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_wb_port")]
    pub port: u16,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_true")]
    pub auto_checkin: bool,
    #[serde(default)]
    pub account: Option<WorkBuddyAccount>,
    /// Legacy multi-account field kept read-only for seamless migration on load.
    #[serde(default, skip_serializing)]
    pub accounts: Vec<WorkBuddyAccount>,
}

impl Default for WorkBuddyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_wb_port(),
            api_key: String::new(),
            auto_checkin: true,
            account: None,
            accounts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub providers: Vec<ProviderConfig>,
    pub active_provider_id: String,
    pub active_model_id: String,
    pub hotkeys: HotkeyConfig,
    #[serde(default)]
    pub mouse_priority: MousePriorityConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub workbuddy: WorkBuddyConfig,
}

fn default_wb_realm() -> String {
    "cn".to_string()
}

fn default_wb_site() -> String {
    "cn".to_string()
}

fn default_wb_port() -> u16 {
    18080
}

fn default_true() -> bool {
    true
}

fn default_freshness() -> u64 {
    90
}

fn default_mouse_toggle() -> String {
    "XButton1/XButton2".to_string()
}

fn default_blacklist() -> Vec<String> {
    Vec::new()
}

fn default_quick_prompts() -> Vec<String> {
    vec![
        "帮我快速总结核心要点".to_string(),
        "翻译成地道中文".to_string(),
        "帮我解释这段内容".to_string(),
    ]
}

impl Default for MousePriorityConfig {
    fn default() -> Self {
        Self {
            passthrough_fullscreen: true,
            blacklist_processes: default_blacklist(),
            disabled_processes: default_blacklist(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_ball: true,
            ball_x: None,
            ball_y: None,
            ball_auto_hide_edge: true,
            popup_auto_hide_on_blur: true,
            clipboard_image_freshness_secs: 90,
            auto_submit_selection: true,
            autostart: false,
            default_web_search: false,
            show_search_sources: false,
            quick_prompts: default_quick_prompts(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            providers: vec![ProviderConfig {
                id: "openai-compat".to_string(),
                name: "OpenAI 兼容接口".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: String::new(),
                models: vec![
                    ModelConfig {
                        id: "gpt-4o-mini".to_string(),
                        label: "GPT-4o mini".to_string(),
                        vision: true,
                        tool_calling: true,
                    },
                    ModelConfig {
                        id: "deepseek-chat".to_string(),
                        label: "DeepSeek Chat".to_string(),
                        vision: false,
                        tool_calling: true,
                    },
                ],
            }],
            active_provider_id: "openai-compat".to_string(),
            active_model_id: "gpt-4o-mini".to_string(),
            hotkeys: HotkeyConfig {
                toggle_popup: "Alt+Space".to_string(),
                mouse_toggle: "XButton1/XButton2".to_string(),
                capture_region: "Alt+A".to_string(),
                capture_window: "Alt+S".to_string(),
            },
            mouse_priority: MousePriorityConfig::default(),
            ui: UiConfig::default(),
            workbuddy: WorkBuddyConfig::default(),
        }
    }
}

pub fn config_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("BTW-Float").join("config.json")
}

fn decrypt_secret_field(field: &mut String) {
    if let Some(enc) = field.strip_prefix("dpapi:") {
        if let Ok(plain) = dpapi_decrypt(enc) {
            *field = plain;
        }
    }
}

fn encrypt_secret_field(field: &mut String) {
    if !field.is_empty() && !field.starts_with("dpapi:") {
        if let Ok(enc) = dpapi_encrypt(field) {
            *field = format!("dpapi:{enc}");
        }
    }
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return AppConfig::default();
    };
    let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&raw) else {
        return AppConfig::default();
    };
    for p in &mut cfg.providers {
        decrypt_secret_field(&mut p.api_key);
    }
    decrypt_secret_field(&mut cfg.workbuddy.api_key);
    if cfg.workbuddy.account.is_none() && !cfg.workbuddy.accounts.is_empty() {
        cfg.workbuddy.account = cfg.workbuddy.accounts.drain(..).next();
    } else {
        cfg.workbuddy.accounts.clear();
    }
    if let Some(acc) = cfg.workbuddy.account.as_mut() {
        decrypt_secret_field(&mut acc.access_token);
        decrypt_secret_field(&mut acc.refresh_token);
        decrypt_secret_field(&mut acc.device_token);
    }
    cfg
}

pub fn save_config(cfg: &AppConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let mut disk_cfg = cfg.clone();
    for p in &mut disk_cfg.providers {
        encrypt_secret_field(&mut p.api_key);
    }
    encrypt_secret_field(&mut disk_cfg.workbuddy.api_key);
    disk_cfg.workbuddy.accounts.clear();
    if let Some(acc) = disk_cfg.workbuddy.account.as_mut() {
        encrypt_secret_field(&mut acc.access_token);
        encrypt_secret_field(&mut acc.refresh_token);
        encrypt_secret_field(&mut acc.device_token);
    }
    let json = serde_json::to_string_pretty(&disk_cfg)
        .map_err(|e| format!("序列化配置失败: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("写入配置失败: {e}"))?;
    Ok(())
}

#[cfg(windows)]
fn dpapi_encrypt(plaintext: &str) -> Result<String, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let bytes = plaintext.as_bytes();
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|e| format!("DPAPI encrypt failed: {e}"))?;
        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let encoded = BASE64.encode(slice);
        let _ = LocalFree(HLOCAL(out_blob.pbData as _));
        Ok(encoded)
    }
}

#[cfg(windows)]
fn dpapi_decrypt(b64: &str) -> Result<String, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut decoded = BASE64
        .decode(b64)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: decoded.len() as u32,
        pbData: decoded.as_mut_ptr(),
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|e| format!("DPAPI decrypt failed: {e}"))?;
        let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
        let text = String::from_utf8_lossy(slice).into_owned();
        let _ = LocalFree(HLOCAL(out_blob.pbData as _));
        Ok(text)
    }
}

#[cfg(not(windows))]
fn dpapi_encrypt(plaintext: &str) -> Result<String, String> {
    Ok(BASE64.encode(plaintext.as_bytes()))
}

#[cfg(not(windows))]
fn dpapi_decrypt(b64: &str) -> Result<String, String> {
    let bytes = BASE64.decode(b64).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
